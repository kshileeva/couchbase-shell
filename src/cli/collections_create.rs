//! The `collections get` command fetches all of the collection names from the server.

use crate::cli::util::{
    cluster_from_conn_str, cluster_identifiers_from, find_org_id, find_project_id,
    get_active_cluster,
};
use crate::client::ManagementRequest::CreateCollection;
use crate::state::{RemoteCapellaOrganization, State};
use log::debug;
use std::ops::Add;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

use crate::cli::collections::{get_bucket_or_active, get_scope_or_active};
use crate::cli::error::{
    client_error_to_shell_error, serialize_error, unexpected_status_code_error,
};
use crate::client::cloud_json::Collection;
use crate::remote_cluster::RemoteCluster;
use crate::remote_cluster::RemoteClusterType::Provisioned;
use nu_engine::CallExt;
use nu_protocol::ast::Call;
use nu_protocol::engine::{Command, EngineState, Stack};
use nu_protocol::{Category, PipelineData, ShellError, Signature, Span, SyntaxShape};

#[derive(Clone)]
pub struct CollectionsCreate {
    state: Arc<Mutex<State>>,
}

impl CollectionsCreate {
    pub fn new(state: Arc<Mutex<State>>) -> Self {
        Self { state }
    }
}

impl Command for CollectionsCreate {
    fn name(&self) -> &str {
        "collections create"
    }

    fn signature(&self) -> Signature {
        Signature::build("collections create")
            .required("name", SyntaxShape::String, "the name of the collection")
            .named(
                "bucket",
                SyntaxShape::String,
                "the name of the bucket",
                None,
            )
            .named("scope", SyntaxShape::String, "the name of the scope", None)
            .named(
                "max-expiry",
                SyntaxShape::Int,
                "the maximum expiry for documents in this collection, in seconds",
                None,
            )
            .named(
                "clusters",
                SyntaxShape::String,
                "the clusters to query against",
                None,
            )
            .category(Category::Custom("couchbase".to_string()))
    }

    fn usage(&self) -> &str {
        "Creates collections through the HTTP API"
    }

    fn run(
        &self,
        engine_state: &EngineState,
        stack: &mut Stack,
        call: &Call,
        input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        collections_create(self.state.clone(), engine_state, stack, call, input)
    }
}

fn collections_create(
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
    let collection: String = call.req(engine_state, stack, 0)?;
    let expiry: i64 = call
        .get_flag(engine_state, stack, "max-expiry")?
        .unwrap_or(0);

    for identifier in cluster_identifiers {
        let active_cluster = get_active_cluster(identifier.clone(), &guard, span)?;

        let bucket = get_bucket_or_active(active_cluster, engine_state, stack, call)?;
        let scope = get_scope_or_active(active_cluster, engine_state, stack, call)?;

        debug!(
            "Running collections create for {:?} on bucket {:?}, scope {:?}",
            &collection, &bucket, &scope
        );

        if active_cluster.cluster_type() == Provisioned {
            create_capella_collection(
                guard.named_or_active_org(active_cluster.capella_org())?,
                guard.named_or_active_project(active_cluster.project())?,
                active_cluster,
                bucket.clone(),
                scope.clone(),
                collection.clone(),
                expiry,
                identifier.clone(),
                ctrl_c.clone(),
                span,
            )
        } else {
            create_server_collection(
                active_cluster,
                scope.clone(),
                bucket.clone(),
                collection.clone(),
                expiry,
                ctrl_c.clone(),
                span,
            )
        }?
    }

    Ok(PipelineData::empty())
}

#[allow(clippy::too_many_arguments)]
fn create_capella_collection(
    org: &RemoteCapellaOrganization,
    project: String,
    cluster: &RemoteCluster,
    bucket: String,
    scope: String,
    collection: String,
    expiry: i64,
    identifier: String,
    ctrl_c: Arc<AtomicBool>,
    span: Span,
) -> Result<(), ShellError> {
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

    let payload = serde_json::to_string(&Collection::new(collection, expiry)).unwrap();

    client
        .create_collection(
            org_id,
            project_id,
            json_cluster.id(),
            bucket,
            scope,
            payload,
            deadline,
            ctrl_c,
        )
        .map_err(|e| client_error_to_shell_error(e, span))
}

fn create_server_collection(
    cluster: &RemoteCluster,
    scope: String,
    bucket: String,
    collection: String,
    expiry: i64,
    ctrl_c: Arc<AtomicBool>,
    span: Span,
) -> Result<(), ShellError> {
    let mut form = vec![("name", collection.clone())];
    if expiry > 0 {
        form.push(("maxTTL", expiry.to_string()));
    }

    let form_encoded =
        serde_urlencoded::to_string(&form).map_err(|e| serialize_error(e.to_string(), span))?;

    let response = cluster
        .cluster()
        .http_client()
        .management_request(
            CreateCollection {
                scope,
                bucket,
                payload: form_encoded,
            },
            Instant::now().add(cluster.timeouts().management_timeout()),
            ctrl_c.clone(),
        )
        .map_err(|e| client_error_to_shell_error(e, span))?;

    match response.status() {
        200 => Ok(()),
        202 => Ok(()),
        _ => {
            return Err(unexpected_status_code_error(
                response.status(),
                response.content(),
                span,
            ));
        }
    }
}
