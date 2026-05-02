mod block;
mod blockchain;
mod transaction;
mod network;
mod api;
pub mod lattice;

use std::env;
use std::sync::{Arc, Mutex};
use std::collections::{HashSet, HashMap}; 
use randomx_rs::{RandomXFlag, RandomXCache, RandomXDataset, RandomXVM};
use blockchain::Blockchain;
use transaction::Transaction;
use api::SharedPool; 

pub type SharedMempool = Arc<Mutex<Vec<Transaction>>>;
pub type SharedPeers = Arc<Mutex<HashSet<String>>>; 

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let is_live_mode = args.contains(&"--live".to_string());
    let clean_args: Vec<String> = args.into_iter().filter(|a| a != "--live").collect();

    if clean_args.len() < 3 {
        eprintln!("🛑 Usage Mineur : cargo run <PORT> <MINER_ADDRESS> [PEER_IP:PORT] [--live]");
        eprintln!("🛡️  Usage Relais : cargo run <PORT> --relay [PEER_IP:PORT] [--live]");
        return;
    }

    let port = clean_args[1].clone();
    let api_port = port.parse::<u16>().unwrap() + 100;
    let arg2 = clean_args[2].clone();
    let is_relay_mode = arg2 == "--relay";
    let miner_address = if is_relay_mode { String::from("RELAY_NODE_NO_MINING") } else { arg2 };
    let peer_target = clean_args.get(3).cloned();

    println!("🔥 DÉMARRAGE DU NŒUD CYPHERPUNK...");
    
    let (p2p_bind_ip, api_bind_ip) = if is_live_mode {
        println!("🌍 MODE LIVE ACTIVÉ : Le Nœud est ouvert sur Internet (0.0.0.0)");
        ("0.0.0.0", [0, 0, 0, 0])
    } else {
        println!("🏠 MODE LOCAL ACTIVÉ : Le Nœud est isolé sur ta machine (127.0.0.1)");
        ("127.0.0.1", [127, 0, 0, 1])
    };

    if is_relay_mode {
        println!("🛡️  MODE RELAIS ACTIVÉ : Minage désactivé. Le Nœud agira comme un routeur P2P.");
    }
    
    let db_file = format!("chain_{}.json", port);
    let shared_chain = Arc::new(Mutex::new(Blockchain::load_from_disk(&db_file)));
    let mempool: SharedMempool = Arc::new(Mutex::new(Vec::new()));
    let dex_pool: SharedPool = Arc::new(Mutex::new(Vec::new()));

    let genesis_hash = shared_chain.lock().unwrap().chain[0].header.hash.clone();
    
    let known_peers: SharedPeers = Arc::new(Mutex::new(HashSet::new()));
    if let Some(target) = &peer_target {
        known_peers.lock().unwrap().insert(target.clone()); 
    }

    // 💡 LE REGISTRE DES TUNNELS P2P ACTIFS
    let active_peers: network::ActivePeers = Arc::new(Mutex::new(HashMap::new()));

    // 🌐 1. DÉMARRAGE DU SERVEUR P2P
    let p2p_chain = Arc::clone(&shared_chain);
    let p2p_mempool = Arc::clone(&mempool);
    let p2p_dex_pool = Arc::clone(&dex_pool);
    let p2p_peers = Arc::clone(&known_peers); 
    let p2p_active = Arc::clone(&active_peers);
    let port_clone = port.clone();
    let bind_ip_p2p = p2p_bind_ip.to_string(); 
    tokio::spawn(async move { network::start_p2p_server(&bind_ip_p2p, &port_clone, p2p_chain, p2p_mempool, p2p_dex_pool, p2p_peers, p2p_active).await; });
    
    // 🔌 2. DÉMARRAGE DE L'API REST
    let api_chain = Arc::clone(&shared_chain);
    let api_mempool = Arc::clone(&mempool);
    let api_peers = Arc::clone(&known_peers); 
    let api_dex_pool = Arc::clone(&dex_pool); // 💡 FIX : On clone le pool DEX avant de l'envoyer dans le thread !
	let api_active_peers = Arc::clone(&active_peers);
    tokio::spawn(async move { api::start_api_server(api_port, api_bind_ip, api_mempool, api_chain, api_peers, api_dex_pool, api_active_peers).await; });

    // 🤝 3. OUVERTURE DU TUNNEL VERS LE BOOTSTRAP
    if let Some(target) = &peer_target {
        println!("🤝 Ouverture du tunnel P2P vers {}...", target);
        let target_clone = target.clone();
        let my_port = port.clone();
        let p2p_chain_handshake = Arc::clone(&shared_chain);
        let p2p_mempool_hs = Arc::clone(&mempool);
        let p2p_dex_hs = Arc::clone(&dex_pool);
        let p2p_peers_hs = Arc::clone(&known_peers);
        let p2p_active_hs = Arc::clone(&active_peers);
        
        tokio::spawn(async move {
            network::connect_to_network(&target_clone, &my_port, p2p_chain_handshake, p2p_mempool_hs, p2p_dex_hs, p2p_peers_hs, p2p_active_hs).await;
        });
    }

    if is_relay_mode {
        let db_file_relay = format!("chain_{}.json", port);
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            let chain = shared_chain.lock().unwrap();
            chain.save_to_disk(&db_file_relay);
        }
    } else {
        println!("\n⚙️  Initialisation du moteur RandomX...");
        let start_rx = std::time::Instant::now(); 
        
        let flags = RandomXFlag::get_recommended_flags();
        let cache = RandomXCache::new(flags, genesis_hash.as_bytes()).unwrap();
        
        println!("⏳ Allocation du Dataset de 2 Go en RAM (Veuillez patienter...)");
        let dataset = RandomXDataset::new(flags, cache.clone(), 0).unwrap();
        let vm = RandomXVM::new(flags, Some(cache), Some(dataset)).unwrap();
        println!("✅ RandomX prêt en {:.2?} !", start_rx.elapsed());

        println!("\n⛏️  Début de l'extraction pour l'adresse : {}...", miner_address);
        loop {

            // --- ÉTAPE A : PRÉPARER LE BLOC ---
            let (mut candidate_block, target) = {
                let mut chain = shared_chain.lock().unwrap();
                let pending_txs = mempool.lock().unwrap().clone();
                chain.prepare_block_template(pending_txs, &miner_address)
            };

            let mut mined = false;

            loop {
                if candidate_block.header.nonce % 2000 == 0 {
                    let chain = shared_chain.lock().unwrap();
                    if chain.chain.len() as u64 > candidate_block.header.index {
                        println!("🛑 [ALERTE] Le réseau a trouvé le Bloc {} avant nous ! Annulation du minage.", candidate_block.header.index);
                        break; 
                    }
                    tokio::task::yield_now().await;
                }

                let header_data = format!("{}{}{}{}", 
                    candidate_block.header.index, 
                    candidate_block.header.timestamp, 
                    candidate_block.header.previous_hash, 
                    candidate_block.header.nonce
                );

                let hash_bytes = vm.calculate_hash(header_data.as_bytes()).unwrap();
                candidate_block.header.hash = hex::encode(&hash_bytes);
                
                let hash_value = num_bigint::BigUint::from_bytes_be(&hash_bytes);

                if hash_value <= target {
                    mined = true;
                    break;
                }
                candidate_block.header.nonce += 1;
            }

            if mined {
                let mut chain = shared_chain.lock().unwrap();
                
                if chain.chain.len() as u64 > candidate_block.header.index {
                     println!("🗑️ [INFO] Hachage trouvé, mais la chaîne a été synchronisée entre temps. Bloc jeté.");
                } 
                else if chain.chain.len() as u64 == candidate_block.header.index {
                    
                    let date_str = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                    let nb_tx = candidate_block.transactions.len();
                    let mut total_fees = 0;
                    
                    for tx in candidate_block.transactions.iter().skip(1) { total_fees += tx.fee; }

                    println!("\n====================================================================");
                    println!("🎉 EURÊKA ! NOUVEAU BLOC FORGÉ PAR LE MINEUR !");
                    println!("====================================================================");
                    println!("📦 Index du Bloc : {}", candidate_block.header.index);
                    println!("🔗 Hash         : {}", candidate_block.header.hash);
                    println!("🕒 Date et Heure : {}", date_str);
                    println!("📝 Transactions  : {} incluses (1 Coinbase + {} Publique/Swap)", nb_tx, nb_tx - 1);
                    println!("💰 Frais perçus  : {} Flames", total_fees);
                    println!("====================================================================\n");
                    
                    for tx in &candidate_block.transactions {
                        if !tx.stealth_address.starts_with("COINBASE_") {
                            chain.spent_key_images.insert(tx.kyber_capsule.clone());
                        }
                    }

                    chain.chain.push(candidate_block.clone());
                    chain.prune_old_signatures(); 
                    chain.update_target(); 
                    chain.save_to_disk(&db_file);

                    // 💡 NOUVEAU : On glisse le bloc dans nos tunnels P2P déjà ouverts !
                    let block_clone = candidate_block.clone();
                    let my_port_clone = port.clone(); 
                    let active_clone = Arc::clone(&active_peers);
                    
                    tokio::spawn(async move {
                        network::broadcast_mined_block(&my_port_clone, block_clone, active_clone).await;
                    });
                }
                
                let mut mp = mempool.lock().unwrap();
                mp.retain(|tx| {
                    !candidate_block.transactions.iter().any(|mined_tx| mined_tx.kyber_capsule == tx.kyber_capsule)
                });
            }
        }
    }
}