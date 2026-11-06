//! `snapreq introspect`: fetch a GraphQL schema and print it as SDL (PLAN P10.3).

use snapreq_core::{Body, Error};
use snapreq_interop::graphql;

use crate::cli::{FormatArg, IntrospectArgs};
use crate::collection_cmd::{self, Target};
use crate::exit;

pub(crate) async fn run(args: &IntrospectArgs) -> u8 {
    let r = &args.request;
    let json = r.format == FormatArg::Json;
    let invalid = |message: String| {
        if json {
            crate::emit_json(&crate::json_out::invalid_arguments(&message));
        } else {
            eprintln!("error: {message}");
        }
        exit::INVALID_REQUEST
    };
    if r.graphql.is_some() || r.data.is_some() || r.json.is_some() || r.export.is_some() {
        return invalid("`snapreq introspect` sends its own query; drop the body options".into());
    }
    let built = collection_cmd::resolve_target(r).and_then(|target| match target {
        Target::Url => crate::send::build_request(r),
        Target::Collection { root, key } => collection_cmd::build(&root, &key, r).map(|b| b.0),
    });
    let mut request = match built {
        Ok(req) => req,
        Err(e) => return invalid(e.to_string()),
    };
    // A GraphQL endpoint takes POST; a request file's method is kept unless it is GET.
    if request.method == http::Method::GET {
        request.method = http::Method::POST;
    }
    request.body = Body::Graphql {
        query: graphql::INTROSPECTION_QUERY.into(),
        variables: None,
        operation_name: Some("IntrospectionQuery".into()),
    };
    let response = match crate::client().execute(&request).await {
        Ok(resp) => resp,
        Err(e) => {
            if json {
                crate::emit_json(&crate::json_out::failure(Some(&request), &e));
            } else {
                eprintln!("error: {e}");
            }
            return match e {
                Error::Execution(ex) if ex.timed_out => exit::TIMEOUT,
                Error::Execution(_) => exit::NETWORK,
                _ => exit::INVALID_REQUEST,
            };
        }
    };
    let answer: serde_json::Value = match serde_json::from_slice(&response.body) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "error: the server answered {} with a body that is not JSON (is this a GraphQL endpoint?)",
                response.status
            );
            return if response.status >= 400 {
                exit::HTTP_ERROR
            } else {
                exit::NETWORK
            };
        }
    };
    match graphql::schema_of(&answer) {
        Ok(schema) => {
            if json {
                crate::emit_json(schema);
            } else {
                print!("{}", graphql::to_sdl(schema));
            }
            exit::OK
        }
        Err(e) => {
            eprintln!("error: {e}");
            if response.status >= 400 {
                exit::HTTP_ERROR
            } else {
                exit::NETWORK
            }
        }
    }
}
