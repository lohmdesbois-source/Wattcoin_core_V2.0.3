use serde::{Serialize, Deserialize};
use crate::transaction::Transaction;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>, 
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHeader {
    pub index: u64,
    pub timestamp: i64,
    pub previous_hash: String,
    pub hash: String,
    pub nonce: u64, 
}

impl Block {
    // 🌍 LE VÉRITABLE GENESIS BLOCK D'OBSCURA
    pub fn genesis() -> Self {
        let header = BlockHeader {
            index: 0,
            timestamp: 1713000000, 
            previous_hash: String::from("0000000000000000000000000000000000000000000000000000000000000000"),
            hash: String::from("GENESIS_HASH_OBSCURA_000000000000000000000000000000000000000000"), // Hash codé en dur
            nonce: 0,
        };

        // 🌍 LE VÉRITABLE GENESIS BLOCK QUANTIQUE D'OBSCURA
        let tx = Transaction {
            stealth_address: "GENESIS".to_string(),
            kyber_capsule: "GENESIS".to_string(),
            aes_vault: "GENESIS".to_string(),
            
            // 🧮 FIX : Engagement mathématique à 0 pour la Genèse
            lattice_commitment: crate::lattice::LatticeCommitment::commit(0, 0), 
            
            fee: 0,
            pq_ring_inputs: vec!["GENESIS".to_string()], 
            dilithium_signature: "GENESIS".to_string(),
        };

        Block {
            header,
            transactions: vec![tx],
        }
    }
}