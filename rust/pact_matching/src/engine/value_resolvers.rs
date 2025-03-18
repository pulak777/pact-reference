//! Structs and traits to resolve values required while executing a plan

use anyhow::anyhow;
use itertools::Itertools;
use serde_json::Value;
use tracing::{instrument, trace};
use pact_models::bodies::OptionalBody;
use pact_models::path_exp::{DocPath, PathToken};
use pact_models::v4::http_parts::HttpRequest;
use pact_models::xml_utils::resolve_matching_node;
use crate::engine::{NodeResult, NodeValue, PlanMatchingContext};

/// Value resolver
pub trait ValueResolver {
  /// Resolve the path expression against the test context
  fn resolve(&self, path: &DocPath, context: &PlanMatchingContext) -> anyhow::Result<NodeValue>;
}

/// Value resolver for an HTTP request
#[derive(Clone, Debug, Default)]
pub struct HttpRequestValueResolver {
  /// Request to resolve values against
  pub request: HttpRequest
}

impl ValueResolver for HttpRequestValueResolver {
  fn resolve(&self, path: &DocPath, _context: &PlanMatchingContext) -> anyhow::Result<NodeValue> {
    if let Some(field) = path.first_field() {
      match field {
        "method" => Ok(NodeValue::STRING(self.request.method.clone())),
        "path" => Ok(NodeValue::STRING(self.request.path.clone())),
        "query" => if path.len() == 2 || (path.len() == 3 && path.is_wildcard()) {
          let qp = self.request.query
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|(k, v)| {
              (k.clone(), v.iter().map(|val| val.clone().unwrap_or_default()).collect())
            })
            .collect();
          Ok(NodeValue::MMAP(qp))
        } else if path.len() == 3 {
          let param_name = path.last_field().unwrap_or_default();
          let qp = self.request.query
            .clone()
            .unwrap_or_default();
          if let Some(val) = qp.get(param_name) {
            let values = val.iter()
              .map(|v| v.clone().unwrap_or_default())
              .collect_vec();
            if values.len() == 1 {
              Ok(NodeValue::STRING(values[0].clone()))
            } else {
              Ok(NodeValue::SLIST(values))
            }
          } else {
            Ok(NodeValue::NULL)
          }
        } else {
          Err(anyhow!("{} is not valid for a HTTP request query parameters", path))
        },
        "headers" => {
          let headers = self.request.headers
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
            .collect();
          if path.len() == 2 || (path.len() == 3 && path.is_wildcard()) {
            Ok(NodeValue::MMAP(headers))
          } else if path.len() == 3 {
            let param_name = path.last_field().unwrap_or_default().to_lowercase();
            if let Some(val) = headers.get(&param_name) {
              if val.len() == 1 {
                Ok(NodeValue::STRING(val[0].clone()))
              } else {
                Ok(NodeValue::SLIST(val.clone()))
              }
            } else {
              Ok(NodeValue::NULL)
            }
          } else if path.len() == 4 && path.last().unwrap_or_default().is_index() {
            let param_name = path.last_field().unwrap_or_default().to_lowercase();
            if let Some(val) = headers.get(&param_name) {
              if let Some(PathToken::Index(index)) = path.last() {
                Ok(NodeValue::STRING(val[index].clone()))
              } else {
                Ok(NodeValue::NULL)
              }
            } else {
              Ok(NodeValue::NULL)
            }
          } else {
            Err(anyhow!("{} is not valid for HTTP request headers", path))
          }
        },
        "content-type" => {
          Ok(self.request.content_type()
            .map(|ct| NodeValue::STRING(ct.to_string()))
            .unwrap_or(NodeValue::NULL))
        },
        "body" if path.len() == 2 => match &self.request.body {
          OptionalBody::Present(bytes, _, _) => Ok(NodeValue::BARRAY(bytes.to_vec())),
          _ => Ok(NodeValue::NULL)
        }
        _ => Err(anyhow!("{} is not valid for a HTTP request", path))
      }
    } else {
      Err(anyhow!("{} is not valid for a HTTP request", path))
    }
  }
}

/// Value resolver for expressions against the current stack value
#[derive(Clone, Debug, Default)]
pub struct CurrentStackValueResolver {}

impl ValueResolver for CurrentStackValueResolver {
  #[instrument(ret, skip_all, fields(%path))]
  fn resolve(&self, path: &DocPath, context: &PlanMatchingContext) -> anyhow::Result<NodeValue> {
    if let Some(result) = context.stack_value() {
      if let NodeResult::VALUE(value) = result {
        match value {
          NodeValue::NULL => {
            Err(anyhow!("Can not resolve '{}', current stack value does not contain a value (is NULL)", path))
          }
          NodeValue::JSON(json) => {
            if path.is_root() {
              Ok(NodeValue::JSON(json))
            } else {
              let json_paths = pact_models::json_utils::resolve_path(&json, path);
              trace!("resolved path {} -> {:?}", path, json_paths);
              if json_paths.is_empty() {
                Ok(NodeValue::NULL)
              } else if json_paths.len() == 1 {
                if let Some(value) = json.pointer(json_paths[0].as_str()) {
                  Ok(NodeValue::JSON(value.clone()))
                } else {
                  Ok(NodeValue::NULL)
                }
              } else {
                let values = json_paths.iter()
                  .map(|path| json.pointer(path.as_str()).cloned().unwrap_or_default())
                  .collect();
                Ok(NodeValue::JSON(Value::Array(values)))
              }
            }
          }
          NodeValue::XML(value) => {
            if path.is_root() {
              Ok(NodeValue::XML(value.clone()))
            } else if let Some(element) = value.as_element() {
              let xml_paths = pact_models::xml_utils::resolve_path(&element, path);
              trace!("resolved path {} -> {:?}", path, xml_paths);
              if xml_paths.is_empty() {
                Ok(NodeValue::NULL)
              } else if xml_paths.len() == 1 {
                if let Some(value) = resolve_matching_node(&element, xml_paths[0].as_str()) {
                  Ok(NodeValue::XML(value.into()))
                } else {
                  Ok(NodeValue::NULL)
                }
              } else {
                let values = xml_paths.iter()
                  .map(|path| {
                    resolve_matching_node(&element, path.as_str())
                      .map(|node| NodeValue::XML(node.into()))
                      .unwrap_or_default()
                  })
                  .collect();
                Ok(NodeValue::LIST(values))
              }
            } else {
              todo!("Deal with other XML types: {}", value)
            }
          }
          _ => {
            Err(anyhow!("Can not resolve '{}', current stack value does not contain a value that is resolvable", path))
          }
        }
      } else {
        Err(anyhow!("Can not resolve '{}', current stack value does not contain a value", path))
      }
    } else {
      Err(anyhow!("Can not resolve '{}', current value stack is either empty or contains an empty value", path))
    }
  }
}

#[cfg(test)]
mod tests {
  use expectest::prelude::*;
  use googletest::prelude::*;
  use maplit::hashmap;
  use rstest::rstest;

  use pact_models::path_exp::DocPath;

  use crate::engine::{NodeValue, PlanMatchingContext};
  use crate::engine::value_resolvers::{HttpRequestValueResolver, ValueResolver};

  #[rstest(
    case("$.method", NodeValue::STRING("GET".to_string())),
    case("$.path", NodeValue::STRING("/".to_string())),
    case("$.query", NodeValue::MMAP(hashmap!{})),
    case("$.headers", NodeValue::MMAP(hashmap!{}))
  )]
  fn http_request_resolve_values(#[case] path: &str, #[case] expected: NodeValue) {
    let path = DocPath::new(path).unwrap();
    let resolver = HttpRequestValueResolver::default();
    let context = PlanMatchingContext::default();
    expect!(resolver.resolve(&path, &context).unwrap()).to(be_equal_to(expected));
  }

  #[googletest::test]
  fn http_request_resolve_failures() {
    let resolver = HttpRequestValueResolver::default();
    let context = PlanMatchingContext::default();

    let path = DocPath::root();
    expect_that!(resolver.resolve(&path, &context), err(displays_as(eq("$ is not valid for a HTTP request"))));

    let path = DocPath::new_unwrap("$.blah");
    expect_that!(resolver.resolve(&path, &context), err(displays_as(eq("$.blah is not valid for a HTTP request"))));
  }
}
