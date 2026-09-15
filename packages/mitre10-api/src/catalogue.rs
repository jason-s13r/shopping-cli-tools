//! The category tree, which lives in the CMS rather than in `/categories`.
//!
//! OCC's `/categories/{code}` answers with one category and its ancestors but
//! never its children, so the hierarchy has to come from somewhere else. The
//! site's own menu is a `CategoryNavigationComponent` embedded in every page
//! response: a nested `navigationNode` whose nodes each point at a
//! `CMSLinkComponent` named `LN_<code>_<Name>`.
//!
//! That means the code and a serviceable name are recoverable from the uids
//! alone. Resolving the link components as well ([`apply`]) replaces the
//! derived names with the site's own and adds each category's URL, which is
//! worth one extra batched call and no more.

use crate::domain::CategoryNode;

/// Build the tree from any page response carrying the navigation component.
///
/// Returns `None` when the page has no such component, which is what a
/// non-page answer or a moved schema looks like.
pub fn tree(page: &serde_json::Value) -> Option<CategoryNode> {
    let root = find_component(page, "CategoryNavigationComponent")?.get("navigationNode")?;
    let children = nodes(root.get("children")?.as_array()?);
    Some(CategoryNode {
        code: String::new(),
        name: "Shop".into(),
        url: Some("/shop".into()),
        children,
    })
}

/// The first component of a type, anywhere in a page response. The CMS nests
/// components in slots in content slots, and the depth is not stable.
fn find_component<'a>(v: &'a serde_json::Value, type_code: &str) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(map) => {
            if map.get("typeCode").and_then(|t| t.as_str()) == Some(type_code) {
                return Some(v);
            }
            map.values().find_map(|v| find_component(v, type_code))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|v| find_component(v, type_code)),
        _ => None,
    }
}

fn nodes(items: &[serde_json::Value]) -> Vec<CategoryNode> {
    items.iter().flat_map(node).collect()
}

/// One navigation node, as zero or more categories.
///
/// Zero or more because the menu is not all categories: `Mitre10DepartmentNavNode`
/// and friends are **structural wrappers** with no code of their own, and the
/// whole department tree hangs off one of them. Dropping such a node would
/// take its children with it, so its children are spliced in where it stood.
fn node(v: &serde_json::Value) -> Vec<CategoryNode> {
    // The link component is the catalogue-bearing half; the node's own uid is
    // a navigation wrapper and only stands in when there is no link.
    let link = v
        .get("entries")
        .and_then(|e| e.as_array())
        .and_then(|e| e.first())
        .and_then(|e| e.get("itemId"))
        .and_then(|i| i.as_str());
    let uid = v.get("uid").and_then(|u| u.as_str());

    let children = v
        .get("children")
        .and_then(|c| c.as_array())
        .map(|cs| nodes(cs))
        .unwrap_or_default();

    let Some((code, derived)) = link.or(uid).and_then(split) else {
        return children;
    };

    vec![CategoryNode {
        code,
        name: v
            .get("title")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .unwrap_or(derived),
        url: None,
        children,
    }]
}

/// `LN_RD2002_Garden_Equipment` -> `("RD2002", "Garden Equipment")`.
///
/// The prefix is the component kind (`LN_` a link, `NN_` a navigation node),
/// the next token is the category code and the rest is the name with
/// underscores for spaces.
///
/// A code must carry a digit. The menu also holds content links spelled
/// `cmsitem_00211010`, and without that rule every one of them becomes a node
/// coded `cmsitem` -- all colliding with each other in [`apply`]'s index.
fn split(uid: &str) -> Option<(String, String)> {
    let rest = uid
        .strip_prefix("LN_")
        .or_else(|| uid.strip_prefix("NN_"))
        .unwrap_or(uid);
    let (code, name) = rest.split_once('_')?;
    if !code.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((code.to_string(), name.replace('_', " ")))
}

/// The link component ids a tree needs resolving, deepest last.
///
/// Feed these to `cms/components` in batches -- the site uses 50 -- and pass
/// the answers to [`apply`].
pub fn link_ids(tree: &CategoryNode) -> Vec<String> {
    let mut ids = Vec::new();
    collect(tree, &mut ids);
    ids
}

fn collect(node: &CategoryNode, ids: &mut Vec<String>) {
    if !node.code.is_empty() {
        ids.push(format!("LN_{}_{}", node.code, node.name.replace(' ', "_")));
    }
    for child in &node.children {
        collect(child, ids);
    }
}

/// Replace derived names with the site's own and fill in each category's URL.
///
/// Matched on the code rather than the whole uid: the uid's name half is
/// generated from the category name and does not always survive a rename, but
/// the code does.
pub fn apply(tree: &mut CategoryNode, components: &serde_json::Value) {
    let resolved = index(components);
    fill(tree, &resolved);
}

type Resolved = std::collections::HashMap<String, (Option<String>, Option<String>)>;

fn index(components: &serde_json::Value) -> Resolved {
    let mut out = Resolved::new();
    let Some(items) = components.get("component").and_then(|c| c.as_array()) else {
        return out;
    };
    for item in items {
        let Some(uid) = item.get("uid").and_then(|u| u.as_str()) else {
            continue;
        };
        let Some((code, _)) = split(uid) else {
            continue;
        };
        out.insert(
            code,
            (
                crate::wire::str_of(item, "linkName"),
                crate::wire::str_of(item, "url"),
            ),
        );
    }
    out
}

fn fill(node: &mut CategoryNode, resolved: &Resolved) {
    if let Some((name, url)) = resolved.get(&node.code) {
        if let Some(name) = name {
            node.name = name.clone();
        }
        if node.url.is_none() {
            node.url.clone_from(url);
        }
    }
    for child in &mut node.children {
        fill(child, resolved);
    }
}

/// Every node that can actually be browsed: the ones whose code names a
/// catalogue level. The top menu entries (`N1`..`N9`) are landing pages and
/// are filtered out, because browsing one matches nothing.
pub fn browsable(tree: &CategoryNode) -> Vec<(String, String, Vec<String>)> {
    let mut out = Vec::new();
    tree.walk(&mut |node, path| {
        if node.level().is_some() {
            out.push((node.code.clone(), node.name.clone(), path.to_vec()));
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page() -> serde_json::Value {
        // The shape the site sends: the component is buried in content slots
        // at a depth that is not worth depending on.
        json!({
            "contentSlots": { "contentSlot": [{
                "components": { "component": [{
                    "uid": "Mitre10CategoryNavComponent",
                    "typeCode": "CategoryNavigationComponent",
                    "navigationNode": {
                        "uid": "Mitre10CategoryNavNode",
                        "children": [{
                            "uid": "NN_N2_Garden",
                            "title": "Garden",
                            "entries": [{ "itemId": "LN_N2_Garden" }],
                            "children": [{
                                "uid": "NN_RD2002_Garden_Equipment",
                                "entries": [{ "itemId": "LN_RD2002_Garden_Equipment" }],
                                "children": [{
                                    "uid": "NN_RS2158_Rugs_Mats",
                                    "entries": [{ "itemId": "LN_RS2158_Rugs_Mats" }],
                                    "children": []
                                }]
                            }]
                        }]
                    }
                }]}
            }]}
        })
    }

    #[test]
    fn the_tree_is_found_however_deep_the_cms_buried_it() {
        let tree = tree(&page()).expect("found");
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].code, "N2");
        assert_eq!(tree.children[0].children[0].code, "RD2002");
    }

    #[test]
    fn a_name_is_derived_from_the_uid_when_the_node_has_no_title() {
        // One page fetch is enough to browse; resolving the link components is
        // an improvement, not a prerequisite.
        let tree = tree(&page()).expect("found");
        assert_eq!(tree.children[0].name, "Garden", "the node's own title wins");
        assert_eq!(
            tree.children[0].children[0].name, "Garden Equipment",
            "derived from the uid when there is no title"
        );
    }

    #[test]
    fn resolving_components_replaces_derived_names_and_adds_urls() {
        let mut tree = tree(&page()).expect("found");
        apply(
            &mut tree,
            &json!({ "component": [
                { "uid": "LN_RD2002_Garden_Equipment", "linkName": "Garden Equipment",
                  "url": "/shop/garden/garden-equipment" },
                { "uid": "LN_RS2158_Rugs_Mats", "linkName": "Rugs & Mats",
                  "url": "/shop/garden/garden-equipment/rugs-mats/c/RS2158" },
            ]}),
        );
        let equipment = &tree.children[0].children[0];
        assert_eq!(
            equipment.url.as_deref(),
            Some("/shop/garden/garden-equipment")
        );
        assert_eq!(
            equipment.children[0].name, "Rugs & Mats",
            "the ampersand only survives the resolved name, not the uid"
        );
    }

    #[test]
    fn only_real_catalogue_categories_are_offered_for_browsing() {
        // `N2` is a landing page. Listing it alongside the rest would invite a
        // browse that matches nothing and reads as an empty category.
        let tree = tree(&page()).expect("found");
        let codes: Vec<String> = browsable(&tree).into_iter().map(|(c, _, _)| c).collect();
        assert_eq!(codes, ["RD2002", "RS2158"]);
    }

    #[test]
    fn a_browsable_node_carries_the_path_that_reaches_it() {
        let tree = tree(&page()).expect("found");
        let rugs = browsable(&tree)
            .into_iter()
            .find(|(code, _, _)| code == "RS2158")
            .expect("present");
        assert_eq!(rugs.2, ["Shop", "Garden", "Garden Equipment"]);
    }

    #[test]
    fn a_uid_splits_into_a_code_and_a_name() {
        assert_eq!(
            split("LN_RD2002_Garden_Equipment"),
            Some(("RD2002".into(), "Garden Equipment".into()))
        );
        assert_eq!(
            split("NN_N1_Building_Hardware"),
            Some(("N1".into(), "Building Hardware".into()))
        );
        assert_eq!(split("DepartmentsCategoryLink"), None, "no code to take");
    }

    #[test]
    fn a_structural_wrapper_hands_its_children_up_rather_than_taking_them_down() {
        // The live menu hangs every department off `Mitre10DepartmentNavNode`,
        // which has no code of its own. Dropping it dropped the whole
        // catalogue and read as "no categories".
        let tree = tree(&json!({
            "typeCode": "CategoryNavigationComponent",
            "navigationNode": { "children": [{
                "uid": "Mitre10DepartmentNavNode",
                "title": "Departments",
                "entries": [{ "itemId": "DepartmentsCategoryLink" }],
                "children": [{
                    "uid": "NN_N2_Garden",
                    "entries": [{ "itemId": "LN_N2_Garden" }],
                    "children": [{
                        "uid": "NN_RD2002_Garden_Equipment",
                        "entries": [{ "itemId": "LN_RD2002_Garden_Equipment" }],
                        "children": []
                    }]
                }]
            }]}
        }))
        .expect("found");

        let codes: Vec<&str> = tree.children.iter().map(|c| c.code.as_str()).collect();
        assert_eq!(
            codes,
            ["N2"],
            "the wrapper is gone and its child stands in its place"
        );
        assert_eq!(tree.children[0].children[0].code, "RD2002");
    }

    #[test]
    fn a_cms_content_link_is_not_mistaken_for_a_category() {
        // The menu carries `cmsitem_00211010` deal links alongside the real
        // categories. Taking `cmsitem` as a code would make every one of them
        // the same node, and the last one resolved would win.
        assert_eq!(split("cmsitem_00211010"), None);

        let tree = tree(&json!({
            "typeCode": "CategoryNavigationComponent",
            "navigationNode": { "children": [
                { "uid": "NN_Deals", "title": "Deals",
                  "entries": [{ "itemId": "cmsitem_00211010" }], "children": [] },
                { "uid": "NN_RD2002_Garden_Equipment",
                  "entries": [{ "itemId": "LN_RD2002_Garden_Equipment" }], "children": [] },
            ]}
        }))
        .expect("found");
        let codes: Vec<&str> = tree.children.iter().map(|c| c.code.as_str()).collect();
        assert_eq!(codes, ["RD2002"]);
    }

    #[test]
    fn a_page_without_the_component_is_none_rather_than_an_empty_tree() {
        // An empty tree would read as "this retailer has no categories"; None
        // says the page did not carry one, which is the actual fact.
        assert!(tree(&json!({ "contentSlots": {} })).is_none());
    }
}
