use crate::block::{Block, BlockHeader};
use crate::transaction::Transaction;
use std::fs;
use num_bigint::BigUint;
use std::collections::HashSet;
use randomx_rs::{RandomXFlag, RandomXCache, RandomXVM};
use rand::seq::SliceRandom;

// ⚙️ CONSTANTES DU RÉSEAU (9 Décimales = Le Giga-Flame)
const FLAME: u64 = 1_000_000_000;
const MATURITY_BLOCKS: u64 = 3; // 🛡️ Délai de sécurité absolu avant Pruning !
const EXPECTED_BLOCK_TIME: u64 = 120; // 🔥 On passe à 600 secondes (10 minutes/bloc)
const INITIAL_REWARD: f64 = 50.0 * (FLAME as f64);    // On commence à 50 Watts / 50 Milliards de Flame
const DECAY_FACTOR: f64 = 0.999999;  // Diminution très lente par bloc (Émission douce)
const TAIL_EMISSION: u64 = 1 * FLAME; // Minimum vital perpétuel pour les mineurs

pub struct Blockchain {
    pub chain: Vec<Block>,
    pub target: BigUint, // 🎯 La Cible en 256 bits
	pub spent_key_images: HashSet<String>, // 🛑 LE REGISTRE NOIR DE LA DOUBLE DÉPENSE
}

impl Blockchain {
    pub fn new() -> Self {
        let max_target = BigUint::from_bytes_be(&[0xFF; 32]);
        let initial_target = &max_target >> 8u32;

        let mut blockchain = Blockchain {
            chain: Vec::new(),
            target: initial_target,
            spent_key_images: HashSet::new(), // Initialisation
        };
        blockchain.chain.push(Block::genesis());
        blockchain
    }
    
    // 1. Charger depuis le disque
    pub fn load_from_disk(filename: &str) -> Self {
        if let Ok(data) = fs::read_to_string(filename) {
            if let Ok(chain) = serde_json::from_str::<Vec<Block>>(&data) {
                println!("💾 HISTORIQUE CHARGÉ : {} blocs retrouvés.", chain.len());
                let max_target = BigUint::from_bytes_be(&[0xFF; 32]);
                
                // 🛑 RECONSTRUCTION DU REGISTRE DES EMPREINTES PQ
                let mut spent_key_images = HashSet::new();
                for block in &chain {
                    for tx in &block.transactions {
                        // On ignore le Genesis et les récompenses de minage
                        if !tx.stealth_address.starts_with("COINBASE_") && tx.stealth_address != "GENESIS" {
                            spent_key_images.insert(tx.kyber_capsule.clone());
                        }
                    }
                }

                let mut blockchain = Blockchain {
                    chain,
                    target: max_target >> 8u32, // Valeur temporaire
                    spent_key_images,
                };
                
                // 🎯 Le Nœud recalcule parfaitement sa difficulté au réveil !
                blockchain.recalculate_target_from_scratch();
                return blockchain;
            }
        }
        println!("🌱 Aucun historique trouvé. Création du Genesis Block...");
        Blockchain::new()
    }
	
    // 2. Sauvegarder sur le disque
    pub fn save_to_disk(&self, filename: &str) {
        let json = serde_json::to_string_pretty(&self.chain).unwrap();
        fs::write(filename, json).expect("Impossible d'écrire sur le disque !");
        println!("💾 Blockchain sauvegardée en toute sécurité dans '{}'.", filename);
    }

    // 3. Prépare le bloc, mais NE LE MINE PAS.
    pub fn prepare_block_template(&mut self, transactions: Vec<Transaction>, miner_address: &str) -> (Block, BigUint) {
        let current_height = self.chain.len() as u64;
        
        println!("\n⏳ Préparation du Bloc {}...", current_height);

        let mut valid_transactions = Vec::new();
		let mut total_fees = 0; // 💰 NOUVEAU : La cagnotte du mineur

        // Filtre des transactions : On ne vérifie plus les montants (le nœud est aveugle), seulement les signatures !
        for tx in transactions {
            if tx.is_valid() { 
                println!("💸 Transaction détectée ! (Pourboire: {} Flames)", tx.fee);
                total_fees += tx.fee; // On ajoute le pourboire à la cagnotte
                valid_transactions.push(tx); 
            }
        }
        
        let previous_block = self.chain.last().unwrap();
        let now = chrono::Utc::now().timestamp();

        let mut time_taken = now - previous_block.header.timestamp;
        if time_taken <= 0 { time_taken = 1; } 
        
        let max_target = BigUint::from_bytes_be(&[0xFF; 32]);
        let initial_target = &max_target >> 8u32; 

        // 🎯 L'affichage de la difficulté (La cible est DÉJÀ à jour grâce à update_target !)
        let difficulty_x100 = (&initial_target * 100u64) / &self.target;
        let diff_int = &difficulty_x100 / 100u64;
        let diff_dec = &difficulty_x100 % 100u64;

        if current_height > 1 { println!("⚙️  Dernier bloc miné en {}s", time_taken); }
        println!("🎯 Difficulté cible : {}.{:02}x", diff_int, diff_dec);

        // 📉 CALCUL DE LA RÉCOMPENSE (En Flames !)
        let mut calculated_reward = (INITIAL_REWARD * DECAY_FACTOR.powf(current_height as f64)) as u64;
        if calculated_reward < TAIL_EMISSION {
            calculated_reward = TAIL_EMISSION;
        }
		
		println!("📉 Émission monétaire : {:.6} Watts", (calculated_reward as f64) / (FLAME as f64));
        
        calculated_reward += total_fees; // 💰 BINGO ! Le mineur empoche les pourboires !

        
		println!("📉 Le mineur, avec le tips : {} Flames gagne en tout : {:.6} Watts", total_fees, (calculated_reward as f64) / (FLAME as f64));

        // 🎁 La transaction de création de monnaie (Coinbase) PQ
        let coinbase_tx = Transaction {
            stealth_address: format!("COINBASE_{}", miner_address), 
            kyber_capsule: "COINBASE_CAPSULE".to_string(),
            aes_vault: calculated_reward.to_string(), 
            
            // 🧮 FIX : Le montant réel (en Flames) mis dans la matrice !
            lattice_commitment: crate::lattice::LatticeCommitment::commit(calculated_reward, 0),
            
            fee: 0,
            pq_ring_inputs: vec!["COINBASE_PUBKEY".to_string()],
            dilithium_signature: "COINBASE_SIG".to_string(),
        };
        valid_transactions.insert(0, coinbase_tx);

        let new_header = BlockHeader {
            index: current_height, 
            timestamp: now,
            previous_hash: previous_block.header.hash.clone(),
            hash: String::new(),
            nonce: 0,
        };

        let block = Block { header: new_header, transactions: valid_transactions };
        (block, self.target.clone())
    }

    // 🪓 Le Fossoyeur : Vide les données inutiles des vieux blocs pour sauver le Disque
    pub fn prune_old_signatures(&mut self) {
        let current_height = self.chain.len() as u64;
        
        // 🛡️ On attend la maturité absolue (100 blocs) avant de tailler !
        if current_height <= MATURITY_BLOCKS { return; }

        let prune_target = current_height - MATURITY_BLOCKS;
        let mut pruned_count = 0;

        for block in &mut self.chain {
            if block.header.index < prune_target {
                for tx in &mut block.transactions {
                    // ✅ On taille la signature Dilithium ET les clés leurres !
                    if tx.dilithium_signature != "PRUNED" && !tx.stealth_address.starts_with("COINBASE_") && tx.stealth_address != "GENESIS" {
                        tx.dilithium_signature.clear(); 
                        tx.dilithium_signature.push_str("PRUNED"); 
                        tx.pq_ring_inputs.clear(); // 🧹 On efface les 11 leurres pour vider la RAM !
                        pruned_count += 1;
                    }
                }
            } else {
                break; 
            }
        }

        if pruned_count > 0 {
            println!("🪓 Pruning automatique (Profondeur {} blocs) : {} signatures quantiques purgées pour sauver l'espace !", MATURITY_BLOCKS, pruned_count);
        }
    }
	
	// 4. Résolution des Forks
    pub fn resolve_fork(&mut self, new_chain: Vec<Block>) -> bool {
        if new_chain.is_empty() || new_chain[0].header.hash != self.chain[0].header.hash { return false; }

        // 🧠 NOUVEAU : Création d'une VM RandomX "Légère" pour la vérification
        let flags = RandomXFlag::get_recommended_flags();
        let genesis_hash = &new_chain[0].header.hash;
        // On crée juste le Cache, PAS le Dataset !
        let cache = RandomXCache::new(flags, genesis_hash.as_bytes()).unwrap();
        // Mode léger : on passe `None` à la place du Dataset
        let vm = RandomXVM::new(flags, Some(cache), None).unwrap(); 

        for i in 1..new_chain.len() {
            let previous_block = &new_chain[i - 1];
            let current_block = &new_chain[i];
            if current_block.header.previous_hash != previous_block.header.hash { return false; }
            
            // 💥 Calcul du hash avec RandomX (Mode Léger)
            let header_data = format!("{}{}{}{}", current_block.header.index, current_block.header.timestamp, current_block.header.previous_hash, current_block.header.nonce);
            let hash_bytes = vm.calculate_hash(header_data.as_bytes()).unwrap();
            let expected_hash = hex::encode(&hash_bytes);

            if current_block.header.hash != expected_hash { return false; }
        }

        self.chain = new_chain;
        self.recalculate_target_from_scratch(); // 🎯 LE VRAI FIX EST ICI ! Le nœud recalcule TOUT.
        true
    }
    
    // 🛡️ LE TRIBUNAL DU RÉSEAU
    pub fn validate_and_add_external_block(&mut self, block: Block) -> Result<(), String> {
        let last_block = self.chain.last().unwrap();

        // 1. Vérification de l'index
        if block.header.index != last_block.header.index + 1 {
            return Err("Index de bloc invalide.".to_string());
        }

        // 2. Vérification du lien cryptographique
        if block.header.previous_hash != last_block.header.hash {
            return Err("Rupture de la chaîne : previous_hash incorrect.".to_string());
        }

        // 3. 💥 Vérification du Hash avec RandomX (Mode Léger)
        let flags = RandomXFlag::get_recommended_flags();
        let genesis_hash = &self.chain[0].header.hash;
        
        let cache = RandomXCache::new(flags, genesis_hash.as_bytes()).map_err(|_| "Erreur Cache RandomX")?;
        let vm = RandomXVM::new(flags, Some(cache), None).map_err(|_| "Erreur VM RandomX")?;

        let header_data = format!("{}{}{}{}", block.header.index, block.header.timestamp, block.header.previous_hash, block.header.nonce);
        let hash_bytes = vm.calculate_hash(header_data.as_bytes()).map_err(|_| "Erreur calcul Hash")?;
        let expected_hash = hex::encode(&hash_bytes);
        
        if block.header.hash != expected_hash {
            return Err("Hash frauduleux : Preuve de travail RandomX invalide.".to_string());
        }

        // 4. Vérification de la Difficulté
        let hash_bigint = BigUint::parse_bytes(block.header.hash.as_bytes(), 16).unwrap_or_default();
        if hash_bigint > self.target {
            return Err("Preuve de travail insuffisante (Hash supérieur à la Target).".to_string());
        }

        // 5. Validation stricte de TOUTES les transactions
        let mut coinbase_count = 0;
        let mut block_key_images = HashSet::new(); 

        for tx in &block.transactions {
            if !tx.is_valid() {
                return Err("Une transaction contient une signature invalide.".to_string());
            }

            if tx.stealth_address.starts_with("COINBASE_") {
                coinbase_count += 1;
                if tx.kyber_capsule != "COINBASE_CAPSULE" { 
                    return Err("Montant Coinbase illégal.".to_string());
                }
            } else if tx.stealth_address != "GENESIS" {
                
                // 🚨 LA LOI CYPHERPUNK : VÉRIFICATION DE MATURITÉ DES INPUTS !
                for input_ref in &tx.pq_ring_inputs {
                    // On cherche à quel moment cet input a été créé dans la blockchain
                    for (height, past_block) in self.chain.iter().enumerate() {
                        if let Some(source_tx) = past_block.transactions.iter().find(|t| t.kyber_capsule == *input_ref || t.stealth_address == *input_ref) {
                            
                            // Si la source de cet argent est un Coinbase...
                            if source_tx.stealth_address.starts_with("COINBASE_") {
                                let age = block.header.index - (height as u64);
                                // Le couperet tombe : Moins de 100 blocs ? REJET IMMÉDIAT.
                                if age < MATURITY_BLOCKS {
                                    return Err(format!("Fraude détectée : Tentative d'utiliser un Coinbase immature (Âge: {} blocs, Requis: {} blocs). Bloc rejeté !", age, MATURITY_BLOCKS));
                                }
                            }
                            break; // On a trouvé la source, on passe au leurre suivant
                        }
                    }
                }

                // 🚨 VÉRIFICATION DE LA DOUBLE DÉPENSE PQ
                if self.spent_key_images.contains(&tx.kyber_capsule) {
                    return Err(format!("Double dépense ! La capsule {} a déjà été utilisée.", tx.kyber_capsule));
                }
                if !block_key_images.insert(tx.kyber_capsule.clone()) {
                    return Err("Double dépense simultanée dans le même bloc !".to_string());
                }
            }
        }

        // 6. Règle de la Coinbase unique
        if coinbase_count != 1 {
            return Err("Le bloc doit contenir exactement UNE transaction Coinbase.".to_string());
        }

        // ✅ VERDICT : COUPABLE DE RIEN.
        // On ajoute toutes les nouvelles empreintes au registre noir pour l'éternité !
        for ki in block_key_images {
            self.spent_key_images.insert(ki);
        }

        println!("✅ Bloc {} validé ! Identités protégées. Empreintes enregistrées.", block.header.index);
        self.chain.push(block);
        self.prune_old_signatures(); 
        
        self.update_target(); // 🎯 AJUSTEMENT DE LA DIFFICULTÉ ICI !

        Ok(())
    }
	
	/// 🎯 Met à jour la Cible (Difficulté) après l'ajout d'un bloc
    pub fn update_target(&mut self) {
        // On récupère la taille de la chaîne en tant que `usize` pour pouvoir indexer le tableau
        let current_len = self.chain.len(); 
        if current_len < 2 { return; }

        // Maintenant, on peut soustraire et utiliser ces valeurs comme index
        let previous_block = &self.chain[current_len - 2];
        let last_block = &self.chain[current_len - 1];
        
        let mut time_taken = last_block.header.timestamp - previous_block.header.timestamp;
        
        // Bouclier Anti-Time Warp
        if time_taken > (EXPECTED_BLOCK_TIME * 5) as i64 { time_taken = (EXPECTED_BLOCK_TIME * 5) as i64; }
        if time_taken <= 0 { time_taken = 1; } 

        let max_target = num_bigint::BigUint::from_bytes_be(&[0xFF; 32]);
        let dampening = 4; 
        let damped_time = (time_taken as u64 + (EXPECTED_BLOCK_TIME * (dampening - 1))) / dampening;
        
        self.target = &self.target * damped_time / EXPECTED_BLOCK_TIME;
        
        if self.target > max_target { self.target = max_target; }
    }
	
	/// 🧠 Recalcule toute la difficulté depuis le Genesis Block
    pub fn recalculate_target_from_scratch(&mut self) {
        let max_target = num_bigint::BigUint::from_bytes_be(&[0xFF; 32]);
        let mut current_target = &max_target >> 8u32; // On part de la Genèse

        // On rejoue le film pour chaque bloc pour trouver la difficulté ACTUELLE
        for i in 2..=self.chain.len() {
            let previous_block = &self.chain[i - 2];
            let last_block = &self.chain[i - 1];
            
            let mut time_taken = last_block.header.timestamp - previous_block.header.timestamp;
            if time_taken > (EXPECTED_BLOCK_TIME * 5) as i64 { time_taken = (EXPECTED_BLOCK_TIME * 5) as i64; }
            if time_taken <= 0 { time_taken = 1; } 

            let dampening = 4; 
            let damped_time = (time_taken as u64 + (EXPECTED_BLOCK_TIME * (dampening - 1))) / dampening;
            
            current_target = &current_target * damped_time / EXPECTED_BLOCK_TIME;
            if current_target > max_target { current_target = max_target.clone(); }
        }

        self.target = current_target;
    }
	
	/// 🏋️‍♂️ Calcule le "Poids" total (Chain Work) d'une chaîne
    pub fn calculate_total_work(chain_to_measure: &[Block]) -> BigUint {
        let max_target = num_bigint::BigUint::from_bytes_be(&[0xFF; 32]);
        let mut current_target = &max_target >> 8u32;
        let mut total_work = num_bigint::BigUint::from(0u32);

        for i in 0..chain_to_measure.len() {
            if i >= 2 {
                let previous_block = &chain_to_measure[i - 2];
                let last_block = &chain_to_measure[i - 1];
                let mut time_taken = last_block.header.timestamp - previous_block.header.timestamp;
                
                if time_taken > (EXPECTED_BLOCK_TIME * 5) as i64 { time_taken = (EXPECTED_BLOCK_TIME * 5) as i64; }
                if time_taken <= 0 { time_taken = 1; } 

                let dampening = 4; 
                let damped_time = (time_taken as u64 + (EXPECTED_BLOCK_TIME * (dampening - 1))) / dampening;
                
                current_target = &current_target * damped_time / EXPECTED_BLOCK_TIME;
                if current_target > max_target { current_target = max_target.clone(); }
            }
            // 💥 LE CALCUL DU POIDS : Plus la cible est petite, plus la fraction est grande !
            total_work += &max_target / &current_target;
        }
        total_work
    }
	
	/// 🥷 Fournit des adresses furtives aléatoires pour servir de leurres (Ring Signatures)
    pub fn get_random_decoys(&self, count: usize) -> Vec<String> {
        let mut all_stealth = Vec::new();
        
        // On récolte toutes les adresses furtives des transactions passées
        for block in &self.chain {
            for tx in &block.transactions {
                // On ignore les transactions système qui n'ont pas de vrai propriétaire caché
                if !tx.stealth_address.starts_with("COINBASE_") && tx.stealth_address != "GENESIS" {
                    all_stealth.push(tx.stealth_address.clone());
                }
            }
        }

        if all_stealth.is_empty() {
            return vec![];
        }

        // On mélange et on en prend 'count'
        let mut rng = rand::thread_rng();
        all_stealth.shuffle(&mut rng);
        all_stealth.into_iter().take(count).collect()
    }
}