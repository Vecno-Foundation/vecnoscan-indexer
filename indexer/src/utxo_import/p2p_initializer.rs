use vecno_p2p_lib::common::ProtocolError;
use vecno_p2p_lib::pb::vecnod_message::Payload;
use vecno_p2p_lib::pb::{VecnodMessage, VersionMessage};
use vecno_p2p_lib::{ConnectionInitializer, IncomingRoute, VecnodHandshake, VecnodMessagePayloadType, Router};
use vecno_indexer_cli::cli_args::CliArgs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender;
use tonic::async_trait;
use uuid::Uuid;

pub struct P2pInitializer {
    cli_args: CliArgs,
    sender: Sender<VecnodMessage>,
}

impl P2pInitializer {
    pub fn new(cli_args: CliArgs, sender: Sender<VecnodMessage>) -> Self {
        P2pInitializer { cli_args, sender }
    }
}

#[async_trait]
impl ConnectionInitializer for P2pInitializer {
    async fn initialize_connection(&self, router: Arc<Router>) -> Result<(), ProtocolError> {
        let mut handshake = VecnodHandshake::new(&router);
        router.start();
        let sender = self.sender.clone();
        let version_msg = handshake.handshake(self.version_message()).await?;
        let mut incoming_route = subscribe_all(&router);
        tokio::spawn(async move {
            while let Some(msg) = incoming_route.recv().await {
                let _ = sender.send(msg).await;
            }
        });
        handshake.exchange_ready_messages().await?;
        self.sender.send(VecnodMessage { request_id: 0, response_id: 0, payload: Some(Payload::Version(version_msg)) }).await.unwrap();
        Ok(())
    }
}

impl P2pInitializer {
    pub fn version_message(&self) -> VersionMessage {
        VersionMessage {
            protocol_version: 7,
            services: 0,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64,
            address: None,
            id: Vec::from(Uuid::new_v4().as_bytes()),
            user_agent: format!("/{}:{}/", env!("CARGO_PKG_NAME"), self.cli_args.version()),
            disable_relay_tx: true,
            subnetwork_id: None,
            network: format!("vecno-{}", self.cli_args.network.to_lowercase()),
        }
    }
}

fn subscribe_all(router: &Arc<Router>) -> IncomingRoute {
    router.subscribe(vec![
        VecnodMessagePayloadType::Addresses,
        VecnodMessagePayloadType::Block,
        VecnodMessagePayloadType::Transaction,
        VecnodMessagePayloadType::BlockLocator,
        VecnodMessagePayloadType::RequestAddresses,
        VecnodMessagePayloadType::RequestRelayBlocks,
        VecnodMessagePayloadType::RequestTransactions,
        VecnodMessagePayloadType::IbdBlock,
        VecnodMessagePayloadType::InvRelayBlock,
        VecnodMessagePayloadType::InvTransactions,
        VecnodMessagePayloadType::Ping,
        VecnodMessagePayloadType::Pong,
        // VecnodMessagePayloadType::Verack,
        // VecnodMessagePayloadType::Version,
        VecnodMessagePayloadType::TransactionNotFound,
        VecnodMessagePayloadType::Reject,
        VecnodMessagePayloadType::PruningPointUtxoSetChunk,
        VecnodMessagePayloadType::RequestIbdBlocks,
        VecnodMessagePayloadType::UnexpectedPruningPoint,
        VecnodMessagePayloadType::IbdBlockLocator,
        VecnodMessagePayloadType::IbdBlockLocatorHighestHash,
        VecnodMessagePayloadType::RequestNextPruningPointUtxoSetChunk,
        VecnodMessagePayloadType::DonePruningPointUtxoSetChunks,
        VecnodMessagePayloadType::IbdBlockLocatorHighestHashNotFound,
        VecnodMessagePayloadType::DoneBlocksWithTrustedData,
        VecnodMessagePayloadType::RequestPruningPointAndItsAnticone,
        VecnodMessagePayloadType::BlockHeaders,
        VecnodMessagePayloadType::RequestNextHeaders,
        VecnodMessagePayloadType::DoneHeaders,
        VecnodMessagePayloadType::RequestPruningPointUtxoSet,
        VecnodMessagePayloadType::RequestHeaders,
        VecnodMessagePayloadType::RequestBlockLocator,
        VecnodMessagePayloadType::PruningPoints,
        VecnodMessagePayloadType::RequestPruningPointProof,
        VecnodMessagePayloadType::PruningPointProof,
        // VecnodMessagePayloadType::Ready,
        VecnodMessagePayloadType::BlockWithTrustedData,
        VecnodMessagePayloadType::TrustedData,
        VecnodMessagePayloadType::RequestIbdChainBlockLocator,
        VecnodMessagePayloadType::IbdChainBlockLocator,
        VecnodMessagePayloadType::RequestAntipast,
        VecnodMessagePayloadType::RequestNextPruningPointAndItsAnticoneBlocks,
    ])
}
