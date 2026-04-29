use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::{Arc, Mutex};
use serde::{Serialize, Deserialize};
use rand::Rng; // 🎲 Pour lancer notre dé
use crate::block::Block;
use crate::blockchain::Blockchain;
use crate::transaction::Transaction;
use crate::api::{Order, SharedPool}; // 🌊 Import de l'Ordre et de la Piscine DEX

#[derive(Serialize, Deserialize, Debug)]
pub enum P2PMessage {
    Handshake { genesis_hash: String, current_height: u64, sender_port: String },
    SyncResponse { blocks: Vec<Block> },
    NewBlock { block: Block, sender_port: String }, 
    
    // 🌼 Les deux états du Pissenlit (Dandelion++)
    WhisperTransaction { tx: Transaction },    // Le Chuchotement (Stem)
    BroadcastTransaction { tx: Transaction },  // Le Cri public (Fluff)
	
	// 🌊 Propager un ordre DEX
    BroadcastOrder { order: Order },
}

// 💡 NOUVEAU : Ajout de host_ip
pub async fn start_p2p_server(host_ip: &str, port: &str, blockchain: Arc<Mutex<Blockchain>>, mempool: Arc<Mutex<Vec<Transaction>>>, dex_pool: SharedPool) {
    let address = format!("{}:{}", host_ip, port);
    let listener = TcpListener::bind(&address).await.unwrap();
    println!("📡 Serveur P2P à l'écoute sur TCP/{} (IP: {})...", port, host_ip);
    
    let my_port = port.to_string(); 

    loop {
        let (mut socket, _) = listener.accept().await.unwrap();
        let blockchain_clone = Arc::clone(&blockchain);
        let mempool_clone = Arc::clone(&mempool); 
        let dex_pool_clone = Arc::clone(&dex_pool); // 💡 LE FIX 1 EST ICI
        let my_port_clone = my_port.clone();

        tokio::spawn(async move {
            let mut buffer = vec![0; 1024 * 1024 * 10]; // 10 Mo
            if let Ok(n) = socket.read(&mut buffer).await {
                if n > 0 {
                    let message_str = String::from_utf8_lossy(&buffer[..n]);
                    
                    if let Ok(message) = serde_json::from_str::<P2PMessage>(&message_str) {
                        let mut chain = blockchain_clone.lock().unwrap();
                        
                        match message {
                            // 🤝 GESTION DU HANDSHAKE
                            P2PMessage::Handshake { genesis_hash, current_height, sender_port } => {
                                let my_genesis = &chain.chain[0].header.hash;
                                if genesis_hash != *my_genesis {
                                    println!("🚨 [P2P] INTRUSION REJETÉE.");
                                } else {
                                    let my_height = chain.chain.len() as u64;
                                    if current_height < my_height {
                                        println!("🔄 [P2P] Le nœud {} est en RETARD. Envoi de l'historique...", sender_port);
                                        let all_blocks = chain.chain.clone();
                                        tokio::spawn(async move {
                                            send_sync_response(&sender_port, all_blocks).await;
                                        });
                                    } else {
                                        println!("🤝 [P2P] Poignée de main ok avec {}.", sender_port);
                                    }
                                }
                            },
                            
                            // 📥 RÉCEPTION DE SYNCHRONISATION
                            P2PMessage::SyncResponse { blocks } => {
                                println!("📦 [P2P] Réception d'une synchronisation massive ({} blocs) !", blocks.len());
                                
                                // 🛡️ LA LOI DU PLUS FORT (HEAVIEST CHAIN RULE)
                                let my_work = Blockchain::calculate_total_work(&chain.chain);
                                let new_work = Blockchain::calculate_total_work(&blocks);
                                
                                if new_work > my_work {
                                    println!("⚖️ Le Juge a pesé : La nouvelle chaîne est plus LOURDE !");
                                    if chain.resolve_fork(blocks) {
                                        println!("✅ Synchronisation réussie ! Nous sommes à jour.");
                                    } else {
                                        println!("❌ Chaîne massive rejetée par le Juge.");
                                    }
                                } else {
                                    println!("🛡️ Chaîne massive ignorée : Notre chaîne est plus lourde ou égale !");
                                }
                            },

                            // 🧱 RÉCEPTION D'UN BLOC EN DIRECT
                            P2PMessage::NewBlock { block, sender_port } => {
                                let my_height = chain.chain.len() as u64;
                                if block.header.index > my_height {
                                    println!("⏩ [P2P] Bloc du futur reçu ({}). Demande de mise à jour !", block.header.index);
                                    let my_genesis = chain.chain[0].header.hash.clone();
                                    let port_for_sync = my_port_clone.clone();
                                    tokio::spawn(async move {
                                        send_handshake(&sender_port, &port_for_sync, my_genesis, my_height).await;
                                    });
                                } else if block.header.index == my_height {
                                    println!("\n🌍 [P2P] Nouveau BLOC {} reçu en direct !", block.header.index);
                                    
                                    // On clone le bloc pour pouvoir l'utiliser dans le nettoyage
                                    let block_to_clean = block.clone(); 

                                    if let Err(e) = chain.validate_and_add_external_block(block) {
                                        println!("   🚨 BLOC REJETÉ : {}", e);
                                    } else {
                                        // 🧹 LE NOUVEAU COUP DE BALAI DU P2P
                                        let mut mp = mempool_clone.lock().unwrap();
                                        mp.retain(|tx| {
                                            !block_to_clean.transactions.iter().any(|mined_tx| mined_tx.kyber_capsule == tx.kyber_capsule)
                                        });
                                        println!("🧹 [MEMPOOL] Nettoyé suite au bloc d'un pair. TX restantes : {}", mp.len());
                                    }
                                }
                            },

                            // 🤫 RÉCEPTION D'UN CHUCHOTEMENT DANDELION
                            P2PMessage::WhisperTransaction { tx } => {
                                let mut rng = rand::thread_rng();
                                let dice_roll = rng.gen_range(1..=10); 

                                if dice_roll <= 2 {
                                    // 💥 FLUFF (20%)
                                    println!("🌼 [DANDELION] Explosion du pissenlit ! Diffusion publique.");
                                    let mut pool = mempool_clone.lock().unwrap();
                                    pool.push(tx.clone());
                                    // (Ici on relaierait le broadcast à tous les voisins connus)
                                } else {
                                    // 🤫 STEM (80%)
                                    println!("🤫 [DANDELION] Relais furtif de la TX...");
                                    // (Ici on relaierait le chuchotement à UN voisin aléatoire)
                                }
                            },

                            // 📢 RÉCEPTION D'UN CRI PUBLIC DANDELION
                            P2PMessage::BroadcastTransaction { tx } => {
                                if tx.is_valid() {
                                    let mut pool = mempool_clone.lock().unwrap();
                                    // On évite les doublons via la signature Dilithium
                                    if !pool.iter().any(|t| t.dilithium_signature == tx.dilithium_signature) {
                                        println!("📥 [MEMPOOL] Nouvelle transaction publique ajoutée.");
                                        pool.push(tx.clone());
                                        // (Ici on continuerait de propager le broadcast)
                                    }
                                }
                            },
							
							// 🌊 RÉCEPTION D'UN ORDRE DEX DU RÉSEAU P2P
                            P2PMessage::BroadcastOrder { order } => {
                                let mut pool = dex_pool_clone.lock().unwrap();
                                // On vérifie qu'on n'a pas déjà cet ordre (Anti-boucle infinie)
                                if !pool.iter().any(|o| o.id == order.id) {
                                    println!("🌊 [P2P DEX] Ordre reçu du réseau : {} {} WATT", order.order_type, order.amount_flames);
                                    pool.push(order.clone());
                                    // Idéalement, on le rediffuserait aux autres voisins ici
                                }
                            },
                        }
                    }
                }
            }
        });
    }
}

// --- FONCTIONS RÉSEAU D'ENVOI ---

pub async fn broadcast_block(target_peer: &str, my_port: &str, block: Block) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::NewBlock { block, sender_port: my_port.to_string() };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
    }
}

pub async fn send_handshake(target_peer: &str, my_port: &str, genesis_hash: String, current_height: u64) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::Handshake { genesis_hash, current_height, sender_port: my_port.to_string() };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
    }
}

pub async fn send_sync_response(target_peer: &str, blocks: Vec<Block>) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::SyncResponse { blocks };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
    }
}

pub async fn broadcast_transaction(target_peer: &str, tx: Transaction) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::BroadcastTransaction { tx };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
    }
}

pub async fn broadcast_order(target_peer: &str, order: Order) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::BroadcastOrder { order };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
    }
}

// 💡 NOUVEAU : Utilitaire magique pour comprendre si on parle à un Nœud local ou distant
fn format_peer_address(target: &str) -> String {
    if target.contains(':') {
        target.to_string() // C'est déjà une IP complète (ex: 198.51.100.42:8000)
    } else {
        format!("127.0.0.1:{}", target) // C'est un test local (ex: 8000)
    }
}