use kuchiki::NodeRef;

use std::{collections::HashMap, path::PathBuf};

pub fn inline_base64(
  mut cache: &mut HashMap<String, Option<String>>,
  config: &super::Config,
  root_path: &PathBuf,
  document: &NodeRef,
) -> crate::Result<()> {
  for target in document
    .select(r#"video, img, link[rel=icon], link[rel="shortcut icon"], link[rel="apple-touch-icon"], link[rel="apple-touch-startup-image"]"#)
    .unwrap()
  {
    let node = target.as_node();
    let element = node.as_element().unwrap();
    let attr = match element.name.local.to_string().as_str() {
      "video" | "img" => "src",
      "link" => "href",
      _ => panic!("tag not implemented"),
    };
    let mut attributes = element.attributes.borrow_mut();
    if let Some(source) = attributes.get(attr) {
      log::debug!("[INLINER] inlining {} on {}", attr, node.to_string());
      if let Some(resolve_source) = crate::get(&mut cache, source, &config, &root_path)? {
        attributes.insert(attr, resolve_source);
      }
    }
  }

  Ok(())
}
