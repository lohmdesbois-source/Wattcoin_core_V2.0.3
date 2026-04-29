use serde::{Serialize, Deserialize};
use pqcrypto_dilithium::dilithium3;
use pqcrypto_traits::sign::{PublicKey, SignedMessage};
use crate::lattice::LatticeCommitment;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub stealth_address: String,      
    pub kyber_capsule: String,        
    pub aes_vault: String,            
    
    // 🧮 L'Engagement Lattice remplace le amount_hash !
    pub lattice_commitment: LatticeCommitment, 
    
    pub fee: u64,                     
    pub pq_ring_inputs: Vec<String>,  
    pub dilithium_signature: String,  
}

impl Transaction {
    pub fn is_valid(&self) -> bool {
        // ⚖️ LA LOI HTLC (ATOMIC SWAP)
        if self.pq_ring_inputs.len() == 1 && self.pq_ring_inputs[0] == "HTLC_CONTRACT" {
            let secret_hex = &self.dilithium_signature;
            let expected_hash = &self.aes_vault; // On a stocké le Hash du contrat ici
            
            // 💡 FIX : Le Tribunal doit DÉCODER l'hexadécimal avant de hasher !
            let secret_bytes = hex::decode(secret_hex).unwrap_or_default();
            let calculated_hash = hex::encode(blake3::hash(&secret_bytes).as_bytes());
            
            if calculated_hash == *expected_hash {
                println!("🔓 [TRIBUNAL] Preuve HTLC mathématiquement validée !");
                return true;
            } else {
                println!("🚨 [TRIBUNAL] Fraude HTLC : Le secret ne correspond pas au Hash !");
                return false;
            }
        }
        // 1. Passe-droit pour la Genèse et le Minage
        if self.stealth_address == "GENESIS" { return true; }
        if self.stealth_address.starts_with("COINBASE_") { return true; }
        if self.dilithium_signature == "PRUNED" { return true; }

        // 🧮 2. VÉRIFICATION HOMOMORPHE LATTICE (Le Graal !)
        // Le réseau calcule l'engagement mathématique des frais, en clair
        let _fee_commitment = LatticeCommitment::commit(self.fee, 0); 
        
        /* Dans une version avec de vraies "Entrées" et "Sorties" multiples, le Nœud ferait ici :
        let sum_inputs = input_commitments.iter().fold(...);
        let sum_outputs = output_commitments.iter().fold(...);
        
        if sum_inputs != sum_outputs.add(&_fee_commitment) {
            println!("🛑 REJET : L'expéditeur essaie de créer de la fausse monnaie !");
            return false;
        }
        */

        // 3. Vérification de la structure de la signature
        let sig_bytes = match hex::decode(&self.dilithium_signature) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let signed_msg = match dilithium3::SignedMessage::from_bytes(&sig_bytes) {
            Ok(s) => s,
            Err(_) => return false,
        };

        // 4. On reconstitue le message (💡 FIX : On utilise c2 du Lattice à la place du amount_hash)
        let tx_data = format!("{}{}{}{}{}", 
            self.stealth_address, 
            self.kyber_capsule, 
            self.aes_vault, 
            self.lattice_commitment.c2, 
            self.fee
        );

        // 🌀 5. L'ÉPREUVE DU CERCLE : Le Nœud essaie la signature contre TOUTES les clés.
        for pubkey_hex in &self.pq_ring_inputs {
            if let Ok(pk_bytes) = hex::decode(pubkey_hex) {
                if let Ok(pk) = dilithium3::PublicKey::from_bytes(&pk_bytes) {
                    if let Ok(opened_msg) = dilithium3::open(&signed_msg, &pk) {
                        if opened_msg == tx_data.as_bytes() {
                            return true; // ✅ Validation anonyme réussie !
                        }
                    }
                }
            }
        }
        
        println!("🛑 REJET : Aucune clé du cercle quantique n'a pu valider la signature !");
        false
    }
}