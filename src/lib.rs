#[macro_use]
extern crate html5ever;

use std::{
  collections::HashMap,
  fs,
  path::{Path, PathBuf},
};

use kuchiki::traits::TendrilSink;
use once_cell::sync::Lazy;
use url::Url;

mod binary;
mod js_css;

static FONT_EXTENSIONS: &[&str] = &[".eot", ".woff2", ".woff", ".tff"];

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
  /// Whether to inline remote content or not.
  pub inline_remote: bool,
  /// Maximum size of files that will be inlined, in bytes
  pub max_inline_size: usize,
}

impl Default for Config {
  /// Enables everything
  fn default() -> Config {
    Config {
      inline_fonts: true,
      inline_remote: true,
      max_inline_size: 5000,
    }
  }
}

fn content_type_map() -> &'static serde_json::Value {
  static MAP: Lazy<serde_json::Value> =
    Lazy::new(|| serde_json::from_str(include_str!("./content-type.json")).unwrap());
  &MAP
}

fn load_path<P: AsRef<Path>>(path: &str, config: &Config, root_path: P) -> Result<Option<String>> {
  if !config.inline_fonts && FONT_EXTENSIONS.iter().any(|f| path.ends_with(f)) {
    log::debug!(
      "[INLINER] `{}` is a font and config.inline_fonts == false",
      path
    );
    return Ok(None);
  }

  let raw = if let Ok(url) = Url::parse(path) {
    if config.inline_remote {
      let response = reqwest::blocking::Client::builder()
        .build()?
        .get(url)
        .send()?;
      if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let content_type = content_type.to_str().unwrap();
        if let Some(extension) = path.split('.').last() {
          let expected_content_type = content_type_map()
            .get(extension)
            .map(|c| c.to_string())
            .unwrap_or_else(|| content_type.to_string());
          if content_type != expected_content_type {
            log::debug!(
              "[INLINER] `{}` response's content type is invalid; expected {} but got {}",
              path,
              expected_content_type,
              content_type,
            );
            return Ok(None);
          }
        }
      }
      Some(response.bytes()?.as_ref().to_vec())
    } else {
      log::debug!(
        "[INLINER] `{}` is a remote URL and config.inline_remote == false",
        path
      );
      None
    }
  } else {
    let file_path = PathBuf::from(path);
    let file_path = if file_path.is_absolute() {
      file_path
    } else {
      root_path.as_ref().to_path_buf().join(file_path)
    };
    log::debug!(
      "[INLINER] loading `{:?}` with fs::read `{:?}`",
      file_path,
      path
    );
    fs::read(file_path).map(|file| Some(file.to_vec()))?
  };
  let res = if let Some(raw) = raw {
    if raw.len() > config.max_inline_size {
      log::debug!(
        "[INLINER] `{}` is greater than the max inline size and will not be inlined",
        path
      );
      None
    } else {
      Some(match path.split('.').last() {
        Some(extension) => {
          if let Some(content_type) = content_type_map().get(extension) {
            log::debug!(
              "[INLINER] encoding `{}` as base64 with content type `{}`",
              path,
              content_type.as_str().unwrap()
            );
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
    }
  } else {
    None
  };
  Ok(res)
}

pub(crate) fn get<P: AsRef<Path>>(
  cache: &mut HashMap<String, Option<String>>,
  path: &str,
  config: &Config,
  root_path: P,
) -> Result<Option<String>> {
  log::debug!("[INLINER] loading {}", path);
  let query_replacer = regex::Regex::new(r"\??#.*").unwrap();
  let path = query_replacer.replace_all(path, "").to_string();
  if path.starts_with("data:") {
    return Ok(None);
  }

  if let Some(res) = cache.get(&path) {
    log::debug!("[INLINER] hit cache on {}", path);
    Ok(res.clone())
  } else {
    match load_path(&path, config, root_path) {
      Ok(res) => {
        cache.insert(path, res.clone());
        Ok(res)
      }
      Err(e) => {
        log::error!("error loading {}: {:?}", path, e);
        Ok(None)
      }
    }
  }
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
  let mut cache = HashMap::new();
  let root_path = root_path.as_ref().canonicalize().unwrap();
  let document = kuchiki::parse_html().one(html);

  binary::inline_base64(&mut cache, &config, &root_path, &document)?;
  js_css::inline_script_link(&mut cache, &config, &root_path, &document)?;

  let html = document.to_string();
  let whitespace_regex = regex::Regex::new(r"( {2,})").unwrap();
  let html = whitespace_regex.replace_all(&html, " ").to_string();

  Ok(html)
}

#[cfg(test)]
mod tests {
  use dissimilar::{diff, Chunk};
  use std::{
    fs::{read, read_to_string},
    io::Write,
    path::PathBuf,
    thread::spawn,
  };
  use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
  use tiny_http::{Header, Response, Server, StatusCode};

  #[cfg(windows)]
  const LINE_ENDING: &str = "\r\n";
  #[cfg(not(windows))]
  const LINE_ENDING: &str = "\n";

  #[test]
  fn match_fixture() {
    env_logger::init();

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures_path = root.join("src/fixtures");

    spawn(move || {
      let server = Server::http("localhost:54321").unwrap();
      for request in server.incoming_requests() {
        let requested = percent_encoding::percent_decode_str(request.url())
          .decode_utf8_lossy()
          .to_string();
        let url: PathBuf = requested.chars().skip(1).collect::<String>().into();
        let file_path = fixtures_path.join(url);
        if let Ok(contents) = read(&file_path) {
          let mut response = Response::from_data(contents);
          let content_type = super::content_type_map()
            .get(file_path.extension().unwrap().to_str().unwrap())
            .map(|c| c.to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
          response.add_header(
            Header::from_bytes(&b"Content-Type"[..], &content_type.as_bytes()[..]).unwrap(),
          );
          request.respond(response).unwrap();
        } else {
          request
            .respond(Response::empty(StatusCode::from(404)))
            .unwrap();
        }
      }
    });

    for file in std::fs::read_dir(root.join(PathBuf::from("src/fixtures"))).unwrap() {
      let path = file.unwrap().path();
      let file_name = path.file_name().unwrap().to_str().unwrap();
      if !file_name.ends_with(".src.html") {
        continue;
      }

      let output = super::inline_file(&path, Default::default())
        .unwrap()
        .replace("\n", LINE_ENDING);

      let expected = read_to_string(
        path
          .parent()
          .unwrap()
          .join(file_name.replace(".src.html", ".result.html")),
      )
      .unwrap();

      if output.replace("\n", " ") != expected.replace("\n", " ") {
        _print_diff(output, expected);
        panic!("test case `{}` failed", file_name.replace(".src.html", ""));
      }
    }
  }

  fn _print_diff(text1: String, text2: String) {
    let difference = diff(&text1, &text2);

    let mut stdout = StandardStream::stdout(ColorChoice::Always);

    for i in 0..difference.len() {
      match difference[i] {
        Chunk::Equal(x) => {
          stdout.reset().unwrap();
          writeln!(stdout, " {}", x).unwrap();
        }
        Chunk::Insert(x) => {
          match difference[i - 1] {
            Chunk::Delete(ref y) => {
              stdout
                .set_color(ColorSpec::new().set_fg(Some(Color::Green)))
                .unwrap();
              write!(stdout, "+").unwrap();
              let diffs = diff(y, x);
              for c in diffs {
                match c {
                  Chunk::Equal(z) => {
                    stdout
                      .set_color(ColorSpec::new().set_fg(Some(Color::Green)))
                      .unwrap();
                    write!(stdout, "{}", z).unwrap();
                    write!(stdout, " ").unwrap();
                  }
                  Chunk::Insert(z) => {
                    stdout
                      .set_color(
                        ColorSpec::new()
                          .set_fg(Some(Color::White))
                          .set_bg(Some(Color::Green)),
                      )
                      .unwrap();
                    write!(stdout, "{}", z).unwrap();
                    stdout.reset().unwrap();
                    write!(stdout, " ").unwrap();
                  }
                  _ => (),
                }
              }
              writeln!(stdout).unwrap();
            }
            _ => {
              stdout
                .set_color(ColorSpec::new().set_fg(Some(Color::Green)))
                .unwrap();
              writeln!(stdout, "+{}", x).unwrap();
            }
          };
        }
        Chunk::Delete(x) => {
          stdout
            .set_color(ColorSpec::new().set_fg(Some(Color::Red)))
            .unwrap();
          writeln!(stdout, "-{}", x).unwrap();
        }
      }
    }
    stdout.reset().unwrap();
    stdout.flush().unwrap();
  }
}
