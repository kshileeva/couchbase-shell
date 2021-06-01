use crate::client::ManagementRequest;
use crate::state::State;
use async_trait::async_trait;
use log::debug;
use nu_engine::CommandArgs;
use nu_errors::ShellError;
use nu_protocol::{Signature, SyntaxShape};
use nu_stream::OutputStream;
use std::ops::Add;
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

pub struct ScopesCreate {
    state: Arc<Mutex<State>>,
}

impl ScopesCreate {
    pub fn new(state: Arc<Mutex<State>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl nu_engine::WholeStreamCommand for ScopesCreate {
    fn name(&self) -> &str {
        "scopes create"
    }

    fn signature(&self) -> Signature {
        Signature::build("scopes create")
            .required_named("name", SyntaxShape::String, "the name of the scope", None)
            .named(
                "bucket",
                SyntaxShape::String,
                "the name of the bucket",
                None,
            )
    }

    fn usage(&self) -> &str {
        "Creates scopes through the HTTP API"
    }

    fn run(&self, args: CommandArgs) -> Result<OutputStream, ShellError> {
        scopes_create(self.state.clone(), args)
    }
}

fn scopes_create(state: Arc<Mutex<State>>, args: CommandArgs) -> Result<OutputStream, ShellError> {
    let ctrl_c = args.ctrl_c();
    let args = args.evaluate_once()?;
    let guard = state.lock().unwrap();

    let scope = match args.call_info.args.get("name") {
        Some(v) => match v.as_string() {
            Ok(uname) => uname,
            Err(e) => return Err(e),
        },
        None => return Err(ShellError::unexpected("name is required")),
    };

    let bucket = match args
        .call_info
        .args
        .get("bucket")
        .map(|bucket| bucket.as_string().ok())
        .flatten()
    {
        Some(v) => v,
        None => match guard.active_cluster().active_bucket() {
            Some(s) => s,
            None => {
                return Err(ShellError::untagged_runtime_error(
                    "Could not auto-select a bucket - please use --bucket instead".to_string(),
                ));
            }
        },
    };

    debug!(
        "Running scope create for {:?} on bucket {:?}",
        &scope, &bucket
    );

    let form = vec![("name", scope)];
    let payload = serde_urlencoded::to_string(&form).unwrap();

    let active_cluster = guard.active_cluster();
    let response = active_cluster.cluster().management_request(
        ManagementRequest::CreateScope { payload, bucket },
        Instant::now().add(active_cluster.timeouts().query_timeout()),
        ctrl_c,
    )?;

    match response.status() {
        200 => Ok(OutputStream::empty()),
        202 => Ok(OutputStream::empty()),
        _ => Err(ShellError::untagged_runtime_error(
            response.content().to_string(),
        )),
    }
}
