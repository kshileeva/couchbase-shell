use crate::cli::buckets_builder::{BucketSettings, JSONBucketSettings};
use crate::cli::buckets_get::bucket_to_nu_value;
use crate::cli::util::{
    cluster_from_conn_str, cluster_identifiers_from, find_org_id, find_project_id,
    get_active_cluster,
};
use crate::client::ManagementRequest;
use crate::state::{RemoteCapellaOrganization, State};
use log::debug;
use std::convert::TryFrom;
use std::ops::Add;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

use crate::cli::error::{
    client_error_to_shell_error, deserialize_error, malformed_response_error,
    unexpected_status_code_error,
};
use crate::remote_cluster::RemoteCluster;
use crate::remote_cluster::RemoteClusterType::Provisioned;
use nu_protocol::ast::Call;
use nu_protocol::engine::{Command, EngineState, Stack};
use nu_protocol::{
    Category, IntoPipelineData, PipelineData, ShellError, Signature, Span, SyntaxShape, Value,
};

#[derive(Clone)]
pub struct Buckets {
    state: Arc<Mutex<State>>,
}

impl Buckets {
    pub fn new(state: Arc<Mutex<State>>) -> Self {
        Self { state }
    }
}

impl Command for Buckets {
    fn name(&self) -> &str {
        "buckets"
    }

    fn signature(&self) -> Signature {
        Signature::build("buckets")
            .named(
                "clusters",
                SyntaxShape::String,
                "the clusters which should be contacted",
                None,
            )
            .category(Category::Custom("couchbase".to_string()))
    }

    fn usage(&self) -> &str {
        "Perform bucket management operations"
    }

    fn run(
        &self,
        engine_state: &EngineState,
        stack: &mut Stack,
        call: &Call,
        input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        buckets_get_all(self.state.clone(), engine_state, stack, call, input)
    }
}

fn buckets_get_all(
    state: Arc<Mutex<State>>,
    engine_state: &EngineState,
    stack: &mut Stack,
    call: &Call,
    _input: PipelineData,
) -> Result<PipelineData, ShellError> {
    let span = call.head;
    let ctrl_c = engine_state.ctrlc.as_ref().unwrap().clone();

    let cluster_identifiers = cluster_identifiers_from(engine_state, stack, &state, call, true)?;
    let guard = state.lock().unwrap();

    debug!("Running buckets");

    let mut results = vec![];
    for identifier in cluster_identifiers {
        let cluster = get_active_cluster(identifier.clone(), &guard, span)?;

        let (buckets, is_cloud) = if cluster.cluster_type() == Provisioned {
            let org = guard.named_or_active_org(cluster.capella_org())?;

            (
                get_capella_buckets(
                    identifier.clone(),
                    org,
                    guard.named_or_active_project(cluster.project())?,
                    cluster,
                    ctrl_c.clone(),
                    span,
                )?,
                true,
            )
        } else {
            (get_server_buckets(cluster, ctrl_c.clone(), span)?, false)
        };

        for bucket in buckets {
            results.push(bucket_to_nu_value(
                bucket,
                identifier.clone(),
                is_cloud,
                span,
            ));
        }
    }

    Ok(Value::List {
        vals: results,
        internal_span: span,
    }
    .into_pipeline_data())
}

pub fn get_server_buckets(
    cluster: &RemoteCluster,
    ctrl_c: Arc<AtomicBool>,
    span: Span,
) -> Result<Vec<BucketSettings>, ShellError> {
    let response = cluster
        .cluster()
        .http_client()
        .management_request(
            ManagementRequest::GetBuckets,
            Instant::now().add(cluster.timeouts().management_timeout()),
            ctrl_c.clone(),
        )
        .map_err(|e| client_error_to_shell_error(e, span))?;

    if response.status() != 200 {
        return Err(unexpected_status_code_error(
            response.status(),
            response.content(),
            span,
        ));
    }

    let content: Vec<JSONBucketSettings> = serde_json::from_str(response.content())
        .map_err(|e| deserialize_error(e.to_string(), span))?;

    let mut buckets: Vec<BucketSettings> = vec![];
    for bucket in content.into_iter() {
        buckets.push(BucketSettings::try_from(bucket).map_err(|e| {
            malformed_response_error(
                "Could not parse bucket settings",
                format!("Error: {}, response content: {}", e, response.content()),
                span,
            )
        })?);
    }

    Ok(buckets)
}

pub fn get_capella_buckets(
    identifier: String,
    org: &RemoteCapellaOrganization,
    project: String,
    cluster: &RemoteCluster,
    ctrl_c: Arc<AtomicBool>,
    span: Span,
) -> Result<Vec<BucketSettings>, ShellError> {
    let client = org.client();
    let deadline = Instant::now().add(org.timeout());

    let org_id = find_org_id(ctrl_c.clone(), &client, deadline, span)?;
    let project_id = find_project_id(
        ctrl_c.clone(),
        project,
        &client,
        deadline,
        span,
        org_id.clone(),
    )?;

    let json_cluster = cluster_from_conn_str(
        identifier.clone(),
        ctrl_c.clone(),
        cluster.hostnames().clone(),
        &client,
        deadline,
        span,
        org_id.clone(),
        project_id.clone(),
    )?;

    let buckets = client
        .list_buckets(org_id, project_id, json_cluster.id(), deadline, ctrl_c)
        .map_err(|e| client_error_to_shell_error(e, span))?;
    let mut buckets_settings: Vec<BucketSettings> = vec![];
    for bucket in buckets.items() {
        buckets_settings.push(BucketSettings::try_from(&bucket).map_err(|e| {
            malformed_response_error(
                "Could not parse bucket settings",
                format!("Error: {}, bucket settings: {:?}", e, bucket),
                span,
            )
        })?);
    }
    Ok(buckets_settings)
}
