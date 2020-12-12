#[macro_use]
extern crate html5ever;

use std::{
  fs,
  path::{Path, PathBuf},
};

use kuchiki::traits::TendrilSink;
use once_cell::sync::Lazy;
use url::Url;

mod binary;
mod script;

/// Inliner error types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// A std::io::ErrorKind::NotFound error with the offending line in the string parameter
  #[error("`{0}`")]
  InvalidPath(String),
  /// Any other file read error that is not NotFound
  #[error("`{0}`")]
  Io(#[from] std::io::Error),
  #[error("http request error: `{0}`")]
  HttpRequest(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Config struct that is passed to `inline_file()` and `inline_html_string()`
///
/// Default enables everything
#[derive(Debug, Copy, Clone)]
pub struct Config {
  /// Whether or not to inline fonts in the css as base64.
  pub inline_fonts: bool,
  /// Replace `\r` and `\r\n` with a space character. Useful to keep line numbers the same in the output to help with debugging.
  pub remove_new_lines: bool,
  /// Whether to inline remote content or not.
  pub inline_remote: bool,
}

impl Default for Config {
  /// Enables everything
  fn default() -> Config {
    Config {
      inline_fonts: true,
      remove_new_lines: true,
      inline_remote: true,
    }
  }
}

fn content_type_map() -> &'static serde_json::Value {
  static MAP: Lazy<serde_json::Value> =
    Lazy::new(|| serde_json::from_str(include_str!("./content-type.json")).unwrap());
  &MAP
}

// TODO cache
pub(crate) fn get<P: AsRef<Path>>(
  path: &str,
  config: &Config,
  root_path: P,
) -> Result<Option<String>> {
  let raw = if let Ok(url) = Url::parse(path) {
    if config.inline_remote {
      let response = reqwest::blocking::get(url)?.bytes()?;
      Some(response.as_ref().to_vec())
    } else {
      None
    }
  } else {
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
      path
    } else {
      root_path.as_ref().to_path_buf().join(path)
    };
    fs::read(path).map(|file| Some(file.to_vec()))?
  };
  let res = if let Some(raw) = raw {
    Some(match path.split('.').last() {
      Some(extension) => {
        if let Some(content_type) = content_type_map().get(extension) {
          format!(
            "data:{};base64,{}",
            content_type.as_str().unwrap(),
            base64::encode(&raw)
          )
        } else {
          String::from_utf8_lossy(&raw).to_string()
        }
      }
      None => String::from_utf8_lossy(&raw).to_string(),
    })
  } else {
    None
  };
  Ok(res)
}

/// Returns a `Result<String>` of the html file at file path with all the assets inlined.
///
/// ## Arguments
/// * `file_path` - The path of the html file.
/// * `config` - Pass a config file to select what features to enable. Use `Default::default()` to enable everything
pub fn inline_file<P: AsRef<Path>>(file_path: P, config: Config) -> Result<String> {
  let html = fs::read_to_string(&file_path)?;
  inline_html_string(&html, &file_path.as_ref().parent().unwrap(), config)
}

/// Returns a `Result<String>` with all the assets linked in the the html string inlined.
///
/// ## Arguments
/// * `html` - The html string.
/// * `root_path` - The root all relative paths in the html will be evaluated with, usually this is the folder the html file is in.
/// * `config` - Pass a config file to select what features to enable. Use `Default::default()` to enable everything
///
pub fn inline_html_string<P: AsRef<Path>>(
  html: &str,
  root_path: P,
  config: Config,
) -> Result<String> {
  // FIXME: make actual error return
  let root_path = root_path.as_ref().canonicalize().unwrap();
  let document = kuchiki::parse_html().one(html);

  binary::inline_base64(&config, &root_path, &document)?;
  script::inline_script_link(&config, &root_path, &document)?;

  let mut html = document.to_string();
  if config.remove_new_lines {
    html = html.replace("\n", " ").replace("\r\n", " ");
  }
  let whitespace_regex = regex::Regex::new(r"(\s{2,})").unwrap();
  html = whitespace_regex.replace_all(&html, " ").to_string();

  Ok(html)
}

#[cfg(test)]
mod tests {
  use std::{
    fs::{read, read_to_string},
    path::PathBuf,
    thread::spawn,
  };
  use tiny_http::{Header, Response, Server};

  #[test]
  fn match_fixture() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures_path = root.join("src/fixtures");

    spawn(move || {
      let server = Server::http("localhost:54321").unwrap();
      for request in server.incoming_requests() {
        let url: PathBuf = request.url().chars().skip(1).collect::<String>().into();
        let file_path = fixtures_path.join(url);
        let contents = read(file_path).unwrap();
        let mut response = Response::from_data(contents);
        response.add_header(
          Header::from_bytes(&b"Content-Type"[..], &b"application/octet-stream"[..]).unwrap(),
        );
        request.respond(response).unwrap();
      }
    });

    let tests = ["css-ext", "css-import"];

    for test_case in &tests {
      let output = super::inline_file(
        root.join(PathBuf::from(format!(
          "src/fixtures/{}.src.html",
          test_case
        ))),
        Default::default(),
      )
      .unwrap();
      let expected = read_to_string(root.join(PathBuf::from(format!(
        "src/fixtures/{}.result.html",
        test_case
      ))))
      .unwrap();
      assert_eq!(output, expected);
    }
  }
}
