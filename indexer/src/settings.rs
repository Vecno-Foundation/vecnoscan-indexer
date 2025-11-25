use vecno_hashes::Hash as VecnoHash;
use serde::{Deserialize, Serialize};
use vecno_indexer_cli::cli_args::CliArgs;
use utoipa::ToSchema;

#[derive(ToSchema, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub cli_args: CliArgs,
    pub net_bps: u8,
    pub net_tps_max: u16,
    #[schema(value_type = String)]
    pub checkpoint: VecnoHash,
    pub disable_vcp_wait_for_sync: bool,
}
