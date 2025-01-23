use vecno_hashes::Hash as VecnoHash;
use sqlx;
use sqlx::encode::IsNull;
use sqlx::postgres::{PgArgumentBuffer, PgHasArrayType, PgTypeInfo, PgValueRef};
use sqlx::{Decode, Encode, Postgres, Type};
use std::fmt::{Display, Formatter};

/// Wrapper type for vecno_hashes::Hash implementing the SQLX Encode & Decode traits
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct Hash(VecnoHash);

impl Hash {
    pub const fn as_bytes(&self) -> [u8; 32] {
        self.0.as_bytes()
    }
}

impl Display for Hash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<VecnoHash> for Hash {
    fn from(hash: VecnoHash) -> Self {
        Hash(hash)
    }
}

impl From<Hash> for VecnoHash {
    fn from(sql_hash: Hash) -> Self {
        sql_hash.0
    }
}

impl Type<Postgres> for Hash {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("BYTEA")
    }
}

impl PgHasArrayType for Hash {
    fn array_type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("_BYTEA")
    }
}

impl Encode<'_, Postgres> for Hash {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, Box<dyn std::error::Error + Send + Sync + 'static>> {
        buf.extend_from_slice(&self.0.as_bytes());
        Ok(IsNull::No)
    }
}

impl<'r> Decode<'r, Postgres> for Hash {
    fn decode(value: PgValueRef<'r>) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let bytes = value.as_bytes()?;
        let vecno_hash = VecnoHash::from_slice(bytes);
        Ok(Hash(vecno_hash))
    }
}
