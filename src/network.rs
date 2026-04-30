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
                        
                        // ❌ ON SUPPRIME le `let mut chain = blockchain_clone.lock().unwrap();` qui était ici !

                        match message {
                            // 🤝 GESTION DU HANDSHAKE
                            P2PMessage::Handshake { genesis_hash, current_height, sender_port } => {
                                let (is_behind, blocks_to_send) = {
                                    let chain = blockchain_clone.lock().unwrap(); // 🔒 Verrou local !
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
                                }; // 🔓 Le verrou saute automatiquement ici !

                                if is_behind {
                                    if let Some(all_blocks) = blocks_to_send {
                                        let envelope = P2PMessage::SyncResponse { blocks: all_blocks };
                                        // ✅ L'AWAIT EST MAINTENANT 100% SÉCURISÉ (Plus de verrou actif)
                                        let _ = socket.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await; 
                                    }
                                }
                            },
                            
                            // 📥 RÉCEPTION DE SYNCHRONISATION
                            P2PMessage::SyncResponse { blocks } => {
                                let mut chain = blockchain_clone.lock().unwrap(); // 🔒 Verrou local
                                println!("📦 [P2P] Réception d'une synchronisation massive ({} blocs) !", blocks.len());
                                
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
                            P2PMessage::NewBlock { block, sender_port: _ } => {
                                let (needs_sync, my_genesis, my_height, process_block) = {
                                    let chain = blockchain_clone.lock().unwrap(); // 🔒 Verrou local
                                    let my_height = chain.chain.len() as u64;
                                    let my_genesis = chain.chain[0].header.hash.clone();
                                    
                                    (block.header.index > my_height, my_genesis, my_height, block.header.index == my_height)
                                }; // 🔓 Le verrou saute ici ! On peut maintenant utiliser le réseau.
                                
                                if needs_sync {
                                    println!("⏩ [P2P] Bloc du futur reçu ({}). Demande de mise à jour !", block.header.index);
                                    
                                    // 💡 FIX NAT (Serveur) : On demande la mise à jour DANS LE MÊME TUYAU TCP !
                                    let envelope = P2PMessage::Handshake { genesis_hash: my_genesis, current_height: my_height, sender_port: my_port_clone.clone() };
                                    let _ = socket.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
                                    
                                    // ⏱️ NOUVEAU : On donne 1 seconde au mineur pour nous renvoyer la lourde blockchain !
                                    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                                    
                                    // On attend la réponse (la blockchain complète) qui va revenir
                                    if let Ok(n2) = socket.read(&mut buffer).await {
                                        if n2 > 0 {
                                            let response_str = String::from_utf8_lossy(&buffer[..n2]);
                                            if let Ok(P2PMessage::SyncResponse { blocks }) = serde_json::from_str::<P2PMessage>(&response_str) {
                                                println!("📦 [P2P] Réception de la chaîne de rattrapage ({} blocs) !", blocks.len());
                                                
                                                let mut chain = blockchain_clone.lock().unwrap(); // 🔒 On reprend le verrou pour écrire
                                                if chain.resolve_fork(blocks) {
                                                    println!("✅ [P2P] Synchronisation de rattrapage réussie !");
                                                }
                                            }
                                        }
                                    }
                                } else if process_block {
                                    println!("\n🌍 [P2P] Nouveau BLOC {} reçu en direct !", block.header.index);
                                    let block_to_clean = block.clone(); 

                                    let mut chain = blockchain_clone.lock().unwrap(); // 🔒 On reprend le verrou pour écrire
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
                }
            }
        });
    }
}

// --- FONCTIONS RÉSEAU D'ENVOI ---
// 💡 FIX NAT (Mineur) : Ajout de l'accès à la blockchain + Sécurité Send
pub async fn broadcast_block(target_peer: &str, my_port: &str, block: Block, blockchain: Arc<Mutex<Blockchain>>) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::NewBlock { block, sender_port: my_port.to_string() };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;

        // 💡 On écoute 1 seconde au cas où le serveur pleurerait qu'il est en retard
        let mut buffer = vec![0; 1024 * 1024 * 10]; // 10 Mo
        if let Ok(n) = stream.read(&mut buffer).await {
            if n > 0 {
                let message_str = String::from_utf8_lossy(&buffer[..n]);
                if let Ok(P2PMessage::Handshake { current_height, .. }) = serde_json::from_str::<P2PMessage>(&message_str) {
                    println!("📡 [NAT] Le serveur est en retard (Hauteur: {}). Envoi de la blockchain complète...", current_height);
                    
                    // 1. On clone la chaîne tout de suite et on lâche le verrou
                    let blocks_to_send = {
                        let chain = blockchain.lock().unwrap();
                        chain.chain.clone()
                    }; // 🔓 Le verrou saute ici !

                    // 2. On attend tranquillement l'envoi réseau sans bloquer le programme
                    let envelope_sync = P2PMessage::SyncResponse { blocks: blocks_to_send };
                    let _ = stream.write_all(serde_json::to_string(&envelope_sync).unwrap().as_bytes()).await;
                }
            }
        }
    }
}

// 💡 NOUVEAU PARAMÈTRE : blockchain
pub async fn send_handshake(target_peer: &str, my_port: &str, genesis_hash: String, current_height: u64, blockchain: Arc<Mutex<Blockchain>>) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::Handshake { genesis_hash, current_height, sender_port: my_port.to_string() };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;

        // 💡 FIX : Le Mineur écoute la réponse du Serveur immédiatement !
        let mut buffer = vec![0; 1024 * 1024 * 10]; // 10 Mo
        if let Ok(n) = stream.read(&mut buffer).await {
            if n > 0 {
                let message_str = String::from_utf8_lossy(&buffer[..n]);
                if let Ok(P2PMessage::SyncResponse { blocks }) = serde_json::from_str::<P2PMessage>(&message_str) {
                    println!("📦 [P2P] Réception de l'historique au démarrage ({} blocs) !", blocks.len());
                    let mut chain = blockchain.lock().unwrap();
                    if chain.resolve_fork(blocks) {
                        println!("✅ [P2P] Synchronisation de démarrage réussie !");
                    }
                }
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

// 💡 NOUVEAU : Utilitaire magique pour comprendre si on parle à un Nœud local ou distant
fn format_peer_address(target: &str) -> String {
    if target.contains(':') {
        target.to_string() // C'est déjà une IP complète (ex: 198.51.100.42:8000)
    } else {
        format!("127.0.0.1:{}", target) // C'est un test local (ex: 8000)
    }
}