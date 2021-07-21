// Copyright (c) The Diem Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::pos::{
    mempool::{
        core_mempool::CoreMempool,
        network::NetworkReceivers,
        shared_mempool::{
            coordinator::{coordinator, gc_coordinator, snapshot_job},
            peer_manager::PeerManager,
            transaction_validator::TransactionValidator,
            types::{SharedMempool, SharedMempoolNotification},
        },
        CommitNotification, ConsensusRequest, SubmissionStatus,
    },
    protocol::network_sender::NetworkSender,
};
use anyhow::Result;
use channel::diem_channel;
use diem_config::{config::NodeConfig, network_id::NodeNetworkId};
use diem_infallible::{Mutex, RwLock};
use diem_types::{
    on_chain_config::OnChainConfigPayload, transaction::SignedTransaction,
};
use futures::channel::{
    mpsc::{self, Receiver, UnboundedSender},
    oneshot,
};
use std::{collections::HashMap, sync::Arc};
use storage_interface::DbReader;
use tokio::runtime::{Builder, Handle, Runtime};

/// Bootstrap of SharedMempool.
/// Creates a separate Tokio Runtime that runs the following routines:
///   - outbound_sync_task (task that periodically broadcasts transactions to
///     peers).
///   - inbound_network_task (task that handles inbound mempool messages and
///     network events).
///   - gc_task (task that performs GC of all expired transactions by
///     SystemTTL).
pub(crate) fn start_shared_mempool(
    executor: &Handle, config: &NodeConfig, mempool: Arc<Mutex<CoreMempool>>,
    network_sender: NetworkSender, network_receivers: NetworkReceivers,
    client_events: mpsc::Receiver<(
        SignedTransaction,
        oneshot::Sender<Result<SubmissionStatus>>,
    )>,
    consensus_requests: mpsc::Receiver<ConsensusRequest>,
    state_sync_requests: mpsc::Receiver<CommitNotification>,
    mempool_reconfig_events: diem_channel::Receiver<(), OnChainConfigPayload>,
    db: Arc<dyn DbReader>, validator: Arc<RwLock<TransactionValidator>>,
    subscribers: Vec<UnboundedSender<SharedMempoolNotification>>,
)
{
    let peer_manager =
        Arc::new(PeerManager::new(config.base.role, config.mempool.clone()));

    let smp = SharedMempool {
        mempool: mempool.clone(),
        config: config.mempool.clone(),
        network_sender,
        db,
        validator,
        peer_manager,
        subscribers,
    };

    executor.spawn(coordinator(
        smp,
        executor.clone(),
        network_receivers,
        client_events,
        consensus_requests,
        state_sync_requests,
        mempool_reconfig_events,
    ));

    executor.spawn(gc_coordinator(
        mempool.clone(),
        config.mempool.system_transaction_gc_interval_ms,
    ));

    executor.spawn(snapshot_job(
        mempool,
        config.mempool.mempool_snapshot_interval_secs,
    ));
}

pub fn bootstrap(
    config: &NodeConfig, db: Arc<dyn DbReader>, network_sender: NetworkSender,
    network_receivers: NetworkReceivers,
    client_events: Receiver<(
        SignedTransaction,
        oneshot::Sender<Result<SubmissionStatus>>,
    )>,
    consensus_requests: Receiver<ConsensusRequest>,
    state_sync_requests: Receiver<CommitNotification>,
    mempool_reconfig_events: diem_channel::Receiver<(), OnChainConfigPayload>,
) -> Runtime
{
    let runtime = Builder::new_multi_thread()
        .thread_name("shared-mem")
        .enable_all()
        .build()
        .expect("[shared mempool] failed to create runtime");
    let mempool = Arc::new(Mutex::new(CoreMempool::new(&config)));
    let validator = Arc::new(RwLock::new(TransactionValidator::new()));
    start_shared_mempool(
        runtime.handle(),
        config,
        mempool,
        network_sender,
        network_receivers,
        client_events,
        consensus_requests,
        state_sync_requests,
        mempool_reconfig_events,
        db,
        validator,
        vec![],
    );
    runtime
}