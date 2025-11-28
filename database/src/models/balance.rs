
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Balance {
    pub script_public_key_address: String,
    pub balance: i64,
}

impl Balance {
    pub fn new(address: String, balance: i64) -> Self {
        Self {
            script_public_key_address: address,
            balance,
        }
    }

    pub fn delta(address: String, delta: i64) -> (String, i64) {
        (address, delta)
    }
}