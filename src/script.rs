use std::{
  collections::HashMap,
  path::{Path, PathBuf},
};

use html5ever::QualName;
use kuchiki::NodeRef;
use regex::Captures;

pub fn inline_script_link(
  mut cache: &mut HashMap<String, Option<String>>,
  config: &super::Config,
  root_path: &PathBuf,
  document: &NodeRef,
) -> crate::Result<()> {
  let mut targets = vec![];
  for target in document.select("script, style, link").unwrap() {
    targets.push(target);
  }

  for target in targets {
    let node = target.as_node();
    let element = node.as_element().unwrap();

    match element.name.local.to_string().as_str() {
      "script" => {
        let text_attr = element.attributes.borrow_mut();
        if let Some(source) = text_attr.get("src") {
          if let Some(script) = crate::get(&mut cache, &source, &config, &root_path)? {
            let replacement_node =
              NodeRef::new_element(QualName::new(None, ns!(html), "script".into()), None);
            replacement_node.append(NodeRef::new_text(script));

            node.insert_after(replacement_node);
            node.detach();
          }
        } else {
          continue;
        }
      }
      "style" => {
        let css = node.text_contents();
        match inline_css(
          &mut cache,
          Some(css),
          root_path
            .clone()
            .into_os_string()
            .into_string()
            .unwrap()
            .as_str(),
          &config,
          &root_path,
        ) {
          Ok(css) => {
            if let Some(css) = css {
              let replacement_node =
                NodeRef::new_element(QualName::new(None, ns!(html), "style".into()), None);
              replacement_node.append(NodeRef::new_text(css));

              node.insert_after(replacement_node);
              node.detach();
            }
          }
          Err(e) => return Err(e),
        }
      }
      "link" => {
        let css_path = {
          let text_attr = element.attributes.borrow_mut();
          let out = if let Some(c) = text_attr
            .get("rel")
            .filter(|rel| *rel == "stylesheet")
            .and(text_attr.get("href"))
          {
            String::from(c)
          } else {
            continue;
          };
          out
        };

        match inline_css_path(&mut cache, &css_path, &config, &root_path) {
          Ok(css) => {
            if let Some(css) = css {
              let replacement_node =
                NodeRef::new_element(QualName::new(None, ns!(html), "style".into()), None);
              replacement_node.append(NodeRef::new_text(css));

              node.insert_after(replacement_node);
              node.detach();
            }
          }
          Err(e) => return Err(e),
        };
      }
      _ => panic!("tag not implemented"),
    }
  }

  Ok(())
}

fn inline_css_path<P: AsRef<Path>>(
  mut cache: &mut HashMap<String, Option<String>>,
  css_path: &str,
  config: &super::Config,
  root_path: P,
) -> crate::Result<Option<String>> {
  let css = crate::get(&mut cache, css_path, &config, &root_path)?.map(|css| compress_css(&css));
  inline_css(&mut cache, css, css_path, &config, &root_path)
}

fn inline_css<P: AsRef<Path>>(
  mut cache: &mut HashMap<String, Option<String>>,
  css: Option<String>,
  css_path: &str,
  config: &super::Config,
  root_path: P,
) -> crate::Result<Option<String>> {
  let comment_remover = regex::Regex::new(r#"/\*[^*]*\*+(?:[^/*][^*]*\*+)*/"#).unwrap();

  let import_finder: regex::Regex = regex::Regex::new(r#"(@import)(\s*.*?);"#).unwrap(); // Finds all @import in the css
  let url_finder = regex::Regex::new(r#"url\s*?\(\s*?["']?([^"')]+?)["']?\s*?\)"#).unwrap(); // Finds all url(path) in the css and makes them relative to the html file

  let mut is_alright: crate::Result<()> = Ok(());

  let css_data = css.map(|resolved_css| {
    let resolved_css = comment_remover.replace_all(&resolved_css, |_: &Captures| "".to_owned());
    let resolved_css = import_finder.replace_all(&resolved_css, |caps: &Captures| {
      let match_url = caps[2].trim().to_string();
      let match_url = if match_url.starts_with("url") {
        match_url.replace("url", "")
      } else {
        match_url
      }
      .replace("'", "")
      .replace("\"", "")
      .replace("}", "")
      .replace("(", "")
      .replace(")", "")
      .replace(";", "");
      let mut match_split = match_url.split(' ');
      let css_url = match_split.next().unwrap();
      let url_path = if let Ok(url) = url::Url::parse(&css_path) {
        url.join(&css_url).unwrap().to_string()
      } else {
        root_path
          .as_ref()
          .join(&css_url)
          .into_os_string()
          .into_string()
          .unwrap()
      };
      match inline_css_path(&mut cache, &url_path, &config, root_path.as_ref()) {
        Ok(out) => {
          if match_split.next().is_some() {
            format!(
              "@media {}{{{}}}",
              match_url.replace(&format!("{} ", css_url), ""),
              out.unwrap_or_else(|| "".to_owned())
            )
          } else {
            out.unwrap_or_else(|| "".to_owned())
          }
        }
        Err(e) => {
          is_alright = Err(e);
          "".to_owned()
        }
      }
    });

    let resolved_css = url_finder.replace_all(&resolved_css, |caps: &Captures| {
      if caps[1].trim().starts_with("data:") {
        return caps[0].to_owned();
      }
      let url_path = if let Ok(url) = url::Url::parse(&css_path) {
        url.join(&caps[1]).unwrap().to_string()
      } else {
        root_path
          .as_ref()
          .to_path_buf()
          .join(&caps[1])
          .into_os_string()
          .into_string()
          .unwrap()
      };
      if let Ok(Some(resolved)) = crate::get(&mut cache, &url_path, &config, &root_path) {
        format!(
          "url('{}')",
          if url_path.ends_with(".css") {
            compress_css(&resolved)
          } else {
            resolved
          }
        )
      } else {
        format!("url('{}')", &caps[1])
      }
    });
    resolved_css.to_string()
  });

  is_alright.map(|_| css_data)
}

fn compress_css(css: &str) -> String {
  let mut css = css.to_string();
  let replaces = &[
    (regex::Regex::new(r"(\s+)").unwrap(), " "),
    (regex::Regex::new(r":(\s+)").unwrap(), ":"),
    (regex::Regex::new(r"/\*.*?\*").unwrap(), ""),
    (regex::Regex::new(r"(\} )").unwrap(), "}"),
    (regex::Regex::new(r"( \{)").unwrap(), "{"),
    (regex::Regex::new(r"(; )").unwrap(), ";"),
    (regex::Regex::new(r"(\n+)").unwrap(), ""),
  ];
  for (regex, replace) in replaces {
    css = regex
      .replace_all(&css, replace.to_string().as_str())
      .to_string();
  }
  css
}
