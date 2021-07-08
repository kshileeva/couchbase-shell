use crate::cli::buckets_create::collected_value_from_error_string;
use crate::cli::cloud_json::JSONCloudDeleteAllowListRequest;
use crate::cli::util::cluster_identifiers_from;
use crate::client::CloudRequest;
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

pub struct AddressesDrop {
    state: Arc<Mutex<State>>,
}

impl AddressesDrop {
    pub fn new(state: Arc<Mutex<State>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl nu_engine::WholeStreamCommand for AddressesDrop {
    fn name(&self) -> &str {
        "addresses drop"
    }

    fn signature(&self) -> Signature {
        Signature::build("addresses drop")
            .required(
                "address",
                SyntaxShape::String,
                "the address to add to allow access",
            )
            .named(
                "clusters",
                SyntaxShape::String,
                "the clusters which should be contacted",
                None,
            )
    }

    fn usage(&self) -> &str {
        "Removes an address to disallow cloud cluster access"
    }

    fn run(&self, args: CommandArgs) -> Result<OutputStream, ShellError> {
        addresses_drop(self.state.clone(), args)
    }
}

fn addresses_drop(state: Arc<Mutex<State>>, args: CommandArgs) -> Result<OutputStream, ShellError> {
    let ctrl_c = args.ctrl_c();
    let address: String = args.req(0)?;

    debug!("Running address drop for {}", &address);

    let cluster_identifiers = cluster_identifiers_from(&state, &args, true)?;
    let guard = state.lock().unwrap();

    let mut results = vec![];
    for identifier in cluster_identifiers {
        let active_cluster = match guard.clusters().get(&identifier) {
            Some(c) => c,
            None => {
                return Err(ShellError::untagged_runtime_error("Cluster not found"));
            }
        };
        if active_cluster.cloud().is_none() {
            return Err(ShellError::unexpected(
                "addresses add can only be used with clusters registered to a cloud control pane",
            ));
        }

        let cloud = guard
            .cloud_for_cluster(active_cluster.cloud().unwrap())?
            .cloud();
        let cluster_id = cloud.find_cluster_id(
            identifier.clone(),
            Instant::now().add(active_cluster.timeouts().query_timeout()),
            ctrl_c.clone(),
        )?;

        let entry = JSONCloudDeleteAllowListRequest::new(address.clone());

        let response = cloud.cloud_request(
            CloudRequest::DeleteAllowListEntry {
                cluster_id,
                payload: serde_json::to_string(&entry)?,
            },
            Instant::now().add(active_cluster.timeouts().query_timeout()),
            ctrl_c.clone(),
        )?;

        match response.status() {
            204 => {}
            _ => {
                results.push(collected_value_from_error_string(
                    identifier.clone(),
                    response.content(),
                ));
            }
        }
    }

    Ok(OutputStream::from(results))
}
