use crate::models::types::hash::Hash;

#[derive(Clone)]
pub struct AddressBalance {
    pub transaction_id: Hash,
    pub address: String,
    pub amount: i64,
}

impl Eq for AddressBalance {}

impl PartialEq for AddressBalance {
    fn eq(&self, other: &Self) -> bool {
        self.transaction_id == other.transaction_id && 
        self.address == other.address && 
        self.amount == other.amount
    }
}

impl std::hash::Hash for AddressBalance {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.transaction_id.hash(state);
        self.address.hash(state);
        self.amount.hash(state);
    }
}


