use serde::{Serialize, Deserialize};
use rand::Rng;

// 🧮 MODULE LATTICE (Réseaux Euclidiens)
// Simulation d'un engagement de type BDLOP (Module-SIS) pour le PoC
// Q est un grand nombre premier utilisé dans les Lattices (ex: Kyber utilise 3329)
const Q: u64 = 8380417; 

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatticeCommitment {
    pub c1: Vec<u64>, // Vecteur de masquage
    pub c2: u64,      // Montant masqué par le bruit
}

impl LatticeCommitment {
    /// 🔒 Le Wallet cache le montant avant l'envoi
    pub fn commit(amount: u64, blinding_factor: u64) -> Self {
        // Dans un vrai Lattice, 'A' est une matrice publique aléatoire partagée par le réseau.
        // Pour le PoC, on simule l'opération matricielle vectorielle.
        let mut rng = rand::thread_rng();
        
        // c1 = A * r mod Q (On simule un vecteur de taille 3)
        let c1 = vec![
            (blinding_factor * rng.gen_range(1..100)) % Q,
            (blinding_factor * rng.gen_range(1..100)) % Q,
            (blinding_factor * rng.gen_range(1..100)) % Q,
        ];

        // c2 = <b, r> + amount mod Q
        let c2 = (blinding_factor * 12345 + amount) % Q;

        LatticeCommitment { c1, c2 }
    }

    /// ➕ Addition Homomorphe (C_total = C1 + C2)
    pub fn add(&self, other: &LatticeCommitment) -> Self {
        let mut new_c1 = Vec::new();
        for i in 0..self.c1.len() {
            new_c1.push((self.c1[i] + other.c1[i]) % Q);
        }
        let new_c2 = (self.c2 + other.c2) % Q;

        LatticeCommitment { c1: new_c1, c2: new_c2 }
    }

    /// ➖ Soustraction Homomorphe (C_reste = C_total - C_depensé)
    pub fn sub(&self, other: &LatticeCommitment) -> Self {
        let mut new_c1 = Vec::new();
        for i in 0..self.c1.len() {
            // Astuce modulo pour éviter les nombres négatifs en Rust
            let val = (self.c1[i] + Q - other.c1[i]) % Q;
            new_c1.push(val);
        }
        let new_c2 = (self.c2 + Q - other.c2) % Q;

        LatticeCommitment { c1: new_c1, c2: new_c2 }
    }
}