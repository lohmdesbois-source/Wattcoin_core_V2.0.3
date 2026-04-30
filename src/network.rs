use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::{Arc, Mutex};
use serde::{Serialize, Deserialize};
use rand::Rng; // 🎲 Pour lancer notre dé
use crate::block::Block;
use crate::blockchain::Blockchain;
use crate::transaction::Transaction;
use crate::api::{Order, SharedPool};

#[derive(Serialize, Deserialize, Debug)]
pub enum P2PMessage {
    Handshake { genesis_hash: String, current_height: u64, sender_port: String },
    SyncResponse { blocks: Vec<Block> },
    NewBlock { block: Block, sender_port: String }, 
    WhisperTransaction { tx: Transaction },    
    BroadcastTransaction { tx: Transaction },  
    BroadcastOrder { order: Order },
}

// 🛡️ NOUVEAU : BOUCLIER ANTI-FRAGMENTATION TCP
// Cette fonction lit le flux réseau morceau par morceau et recoud le JSON !
async fn read_p2p_message(stream: &mut TcpStream) -> Option<P2PMessage> {
    let mut json_str = String::new();
    let mut temp_buf = vec![0; 65536]; // On lit par gros paquets
    
    while let Ok(n) = stream.read(&mut temp_buf).await {
        if n == 0 { break; } // Le voisin a raccroché
        json_str.push_str(&String::from_utf8_lossy(&temp_buf[..n]));
        
        // On essaie de comprendre le texte. Si ça marche, c'est qu'on a tous les morceaux !
        if let Ok(message) = serde_json::from_str::<P2PMessage>(&json_str) {
            return Some(message);
        }
    }
    None // Le JSON était illisible ou la connexion a coupé
}

pub async fn start_p2p_server(host_ip: &str, port: &str, blockchain: Arc<Mutex<Blockchain>>, mempool: Arc<Mutex<Vec<Transaction>>>, dex_pool: SharedPool) {
    let address = format!("{}:{}", host_ip, port);
    let listener = TcpListener::bind(&address).await.unwrap();
    println!("📡 Serveur P2P à l'écoute sur TCP/{} (IP: {})...", port, host_ip);
    
    let my_port = port.to_string(); 

    loop {
        let (mut socket, _) = listener.accept().await.unwrap();
        let blockchain_clone = Arc::clone(&blockchain);
        let mempool_clone = Arc::clone(&mempool); 
        let dex_pool_clone = Arc::clone(&dex_pool);
        let my_port_clone = my_port.clone();

        tokio::spawn(async move {
            // 💡 FIX : On remplace le vieux `socket.read` par notre bouclier robuste
            if let Some(message) = read_p2p_message(&mut socket).await {
                match message {
                    // 🤝 GESTION DU HANDSHAKE
                    P2PMessage::Handshake { genesis_hash, current_height, sender_port } => {
                        let (is_behind, blocks_to_send) = {
                            let chain = blockchain_clone.lock().unwrap(); 
                            let my_genesis = &chain.chain[0].header.hash;
                            
                            if genesis_hash != *my_genesis {
                                println!("🚨 [P2P] INTRUSION REJETÉE.");
                                (false, None)
                            } else {
                                let my_height = chain.chain.len() as u64;
                                if current_height < my_height {
                                    println!("🔄 [P2P] Le nœud {} est en RETARD. Envoi direct de l'historique...", sender_port);
                                    (true, Some(chain.chain.clone()))
                                } else {
                                    println!("🤝 [P2P] Poignée de main ok avec {}.", sender_port);
                                    (false, None)
                                }
                            }
                        }; 

                        if is_behind {
                            if let Some(all_blocks) = blocks_to_send {
                                let envelope = P2PMessage::SyncResponse { blocks: all_blocks };
                                let _ = socket.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await; 
                            }
                        }
                    },
                    
                    // 📥 RÉCEPTION DE SYNCHRONISATION
                    P2PMessage::SyncResponse { blocks } => {
                        let mut chain = blockchain_clone.lock().unwrap(); 
                        println!("📦 [P2P] Réception d'une synchronisation massive ({} blocs) !", blocks.len());
                        
                        let my_work = Blockchain::calculate_total_work(&chain.chain);
                        let new_work = Blockchain::calculate_total_work(&blocks);
                        
                        if new_work > my_work {
                            println!("⚖️ Le Juge a pesé : La nouvelle chaîne est plus LOURDE (Poids supérieur) !");
                            if chain.resolve_fork(blocks) {
                                println!("✅ Synchronisation réussie ! Nous sommes à jour.");
                            } else {
                                println!("❌ Chaîne massive rejetée par le Juge (Invalide).");
                            }
                        } else if new_work == my_work && blocks.len() > 0 && chain.chain.len() > 0 {
                            // 💡 NOUVEAU FIX : Égalité parfaite de poids ! On départage avec le chronomètre.
                            let my_last_time = chain.chain.last().unwrap().header.timestamp;
                            let new_last_time = blocks.last().unwrap().header.timestamp;
                            
                            if new_last_time < my_last_time {
                                println!("⏱️ Égalité de poids, mais la chaîne concurrente a été minée AVANT la nôtre ! On l'adopte.");
                                if chain.resolve_fork(blocks) {
                                    println!("✅ Synchronisation réussie ! Nous sommes à jour.");
                                } else {
                                    println!("❌ Chaîne concurrente rejetée par le Juge (Invalide).");
                                }
                            } else {
                                println!("🛡️ Égalité parfaite, mais notre chaîne a été minée avant (ou en même temps). On garde la nôtre !");
                            }
                        } else {
                            println!("🛡️ Chaîne massive ignorée : Notre chaîne est plus lourde !");
                        }
                    },

                    // 🧱 RÉCEPTION D'UN BLOC EN DIRECT
                    P2PMessage::NewBlock { block, sender_port: _ } => {
                        let (needs_sync, my_genesis, my_height, process_block) = {
                            let chain = blockchain_clone.lock().unwrap(); 
                            let my_height = chain.chain.len() as u64;
                            let my_genesis = chain.chain[0].header.hash.clone();
                            
                            (block.header.index > my_height, my_genesis, my_height, block.header.index == my_height)
                        }; 
                        
                        if needs_sync {
                            println!("⏩ [P2P] Bloc du futur reçu ({}). Demande de mise à jour !", block.header.index);
                            
                            let envelope = P2PMessage::Handshake { genesis_hash: my_genesis, current_height: my_height, sender_port: my_port_clone.clone() };
                            let _ = socket.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
                            
                            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                            
                            // 💡 FIX : Bouclier anti-fragmentation pour la réponse aussi
                            if let Some(P2PMessage::SyncResponse { blocks }) = read_p2p_message(&mut socket).await {
                                println!("📦 [P2P] Réception de la chaîne de rattrapage ({} blocs) !", blocks.len());
                                let mut chain = blockchain_clone.lock().unwrap(); 
                                if chain.resolve_fork(blocks) {
                                    println!("✅ [P2P] Synchronisation de rattrapage réussie !");
                                }
                            }
                        } else if process_block {
                            println!("\n🌍 [P2P] Nouveau BLOC {} reçu en direct !", block.header.index);
                            let block_to_clean = block.clone(); 

                            let mut chain = blockchain_clone.lock().unwrap();
                            if let Err(e) = chain.validate_and_add_external_block(block) {
                                println!("   🚨 BLOC REJETÉ : {}", e);
                            } else {
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
                            println!("🌼 [DANDELION] Explosion du pissenlit ! Diffusion publique.");
                            let mut pool = mempool_clone.lock().unwrap();
                            pool.push(tx.clone());
                        } else {
                            println!("🤫 [DANDELION] Relais furtif de la TX...");
                        }
                    },

                    // 📢 RÉCEPTION D'UN CRI PUBLIC DANDELION
                    P2PMessage::BroadcastTransaction { tx } => {
                        if tx.is_valid() {
                            let mut pool = mempool_clone.lock().unwrap();
                            if !pool.iter().any(|t| t.dilithium_signature == tx.dilithium_signature) {
                                println!("📥 [MEMPOOL] Nouvelle transaction publique ajoutée.");
                                pool.push(tx.clone());
                            }
                        }
                    },
                    
                    // 🌊 RÉCEPTION D'UN ORDRE DEX DU RÉSEAU P2P
                    P2PMessage::BroadcastOrder { order } => {
                        let mut pool = dex_pool_clone.lock().unwrap();
                        if !pool.iter().any(|o| o.id == order.id) {
                            println!("🌊 [P2P DEX] Ordre reçu du réseau : {} {} WATT", order.order_type, order.amount_flames);
                            pool.push(order.clone());
                        }
                    },
                }
            }
        });
    }
}

// --- FONCTIONS RÉSEAU D'ENVOI ---

pub async fn broadcast_block(target_peer: &str, my_port: &str, block: Block, blockchain: Arc<Mutex<Blockchain>>) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::NewBlock { block, sender_port: my_port.to_string() };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;

        // 💡 FIX : Bouclier anti-fragmentation
        if let Some(P2PMessage::Handshake { current_height, .. }) = read_p2p_message(&mut stream).await {
            println!("📡 [NAT] Le serveur est en retard (Hauteur: {}). Envoi de la blockchain complète...", current_height);
            let blocks_to_send = {
                let chain = blockchain.lock().unwrap();
                chain.chain.clone()
            }; 
            let envelope_sync = P2PMessage::SyncResponse { blocks: blocks_to_send };
            let _ = stream.write_all(serde_json::to_string(&envelope_sync).unwrap().as_bytes()).await;
        }
    }
}

pub async fn send_handshake(target_peer: &str, my_port: &str, genesis_hash: String, current_height: u64, blockchain: Arc<Mutex<Blockchain>>) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::Handshake { genesis_hash, current_height, sender_port: my_port.to_string() };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;

        // 💡 FIX : Bouclier anti-fragmentation
        if let Some(P2PMessage::SyncResponse { blocks }) = read_p2p_message(&mut stream).await {
            println!("📦 [P2P] Réception de l'historique au démarrage ({} blocs) !", blocks.len());
            let mut chain = blockchain.lock().unwrap();
            if chain.resolve_fork(blocks) {
                println!("✅ [P2P] Synchronisation de démarrage réussie !");
            }
        }
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

fn format_peer_address(target: &str) -> String {
    if target.contains(':') {
        target.to_string() 
    } else {
        format!("127.0.0.1:{}", target) 
    }
}