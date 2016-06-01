#[allow(unused_imports)]
use pact_matching::models::Request;
use pact_matching::match_request;
use rustc_serialize::json::Json;
use expectest::prelude::*;
mod body;
mod headers;
mod method;
mod path;
mod query;
