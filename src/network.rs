use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::{Arc, Mutex};
use serde::{Serialize, Deserialize};
use rand::Rng; 
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
    
    // 💡 NOUVEAU : Les messages pour aspirer le Mempool à travers les pare-feux !
    GetMempool,
    MempoolSync { txs: Vec<Transaction> },
}

async fn read_p2p_message(stream: &mut TcpStream) -> Option<P2PMessage> {
    let mut json_str = String::new();
    let mut temp_buf = vec![0; 65536]; 
    
    while let Ok(n) = stream.read(&mut temp_buf).await {
        if n == 0 { break; } 
        json_str.push_str(&String::from_utf8_lossy(&temp_buf[..n]));
        if let Ok(message) = serde_json::from_str::<P2PMessage>(&json_str) {
            return Some(message);
        }
    }
    None 
}

// 💡 FIX : On ajoute known_peers dans les paramètres
pub async fn start_p2p_server(host_ip: &str, port: &str, blockchain: Arc<Mutex<Blockchain>>, mempool: Arc<Mutex<Vec<Transaction>>>, dex_pool: SharedPool, known_peers: crate::SharedPeers) {
    let address = format!("{}:{}", host_ip, port);
    let listener = TcpListener::bind(&address).await.unwrap();
    println!("📡 Serveur P2P à l'écoute sur TCP/{} (IP: {})...", port, host_ip);
    
    let my_port = port.to_string(); 

    loop {
        // 💡 NOUVEAU : On récupère l'IP réelle de la personne qui se connecte
        let (mut socket, peer_addr) = listener.accept().await.unwrap();
        let peer_ip = peer_addr.ip().to_string();
        
        let blockchain_clone = Arc::clone(&blockchain);
        let mempool_clone = Arc::clone(&mempool); 
        let dex_pool_clone = Arc::clone(&dex_pool);
        let peers_clone = Arc::clone(&known_peers);
        let my_port_clone = my_port.clone();

        tokio::spawn(async move {
            if let Some(message) = read_p2p_message(&mut socket).await {
                match message {
                    P2PMessage::Handshake { genesis_hash, current_height, sender_port } => {
                        // 📖 On enregistre ce nouveau contact dans notre carnet !
                        let full_addr = format!("{}:{}", peer_ip, sender_port);
                        peers_clone.lock().unwrap().insert(full_addr);

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
                        
                        // Variable pour savoir si on a accepté la nouvelle chaîne
                        let mut chain_accepted = false; 
                        
                        if new_work > my_work {
                            println!("⚖️ Le Juge a pesé : La nouvelle chaîne est plus LOURDE (Poids supérieur) !");
                            if chain.resolve_fork(blocks.clone()) { // On clone pour pouvoir nettoyer le mempool après
                                println!("✅ Synchronisation réussie ! Nous sommes à jour.");
                                chain_accepted = true;
                            } else {
                                println!("❌ Chaîne massive rejetée par le Juge (Invalide).");
                            }
                        } else if new_work == my_work && blocks.len() > 0 && chain.chain.len() > 0 {
                            let my_last_time = chain.chain.last().unwrap().header.timestamp;
                            let new_last_time = blocks.last().unwrap().header.timestamp;
                            
                            if new_last_time < my_last_time {
                                println!("⏱️ Égalité de poids, mais la chaîne concurrente a été minée AVANT la nôtre ! On l'adopte.");
                                if chain.resolve_fork(blocks.clone()) {
                                    println!("✅ Synchronisation réussie ! Nous sommes à jour.");
                                    chain_accepted = true;
                                } else {
                                    println!("❌ Chaîne concurrente rejetée par le Juge (Invalide).");
                                }
                            } else {
                                println!("🛡️ Égalité parfaite, mais notre chaîne a été minée avant (ou en même temps). On garde la nôtre !");
                            }
                        } else {
                            println!("🛡️ Chaîne massive ignorée : Notre chaîne est plus lourde !");
                        }
                        
                        // 🧹 LE GRAND NETTOYAGE : Si on a adopté une nouvelle chaîne, on vide les transactions
                        // de notre Mempool qui sont DÉJÀ dans cette nouvelle chaîne !
                        if chain_accepted {
                            let mut mp = mempool_clone.lock().unwrap();
                            let initial_len = mp.len();
                            mp.retain(|tx| {
                                // On garde la transaction SEULEMENT SI elle n'est dans aucun des nouveaux blocs
                                !blocks.iter().any(|b| b.transactions.iter().any(|mined_tx| mined_tx.kyber_capsule == tx.kyber_capsule))
                            });
                            let removed = initial_len - mp.len();
                            if removed > 0 {
                                println!("🧹 [MEMPOOL] Nettoyage massif après synchro. {} TX retirée(s).", removed);
                            }
                        }
                    },

                    P2PMessage::NewBlock { block, sender_port } => {
                        let full_addr = format!("{}:{}", peer_ip, sender_port);
                        
                        // 💡 FIX : On donne un CLONE de full_addr au HashSet
                        peers_clone.lock().unwrap().insert(full_addr.clone());

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
                            
                            if let Some(P2PMessage::SyncResponse { blocks }) = read_p2p_message(&mut socket).await {
                                println!("📦 [P2P] Réception de la chaîne de rattrapage ({} blocs) !", blocks.len());
                                let mut chain = blockchain_clone.lock().unwrap(); 
                                if chain.resolve_fork(blocks.clone()) { // 💡 FIX : On clone 'blocks' ici
                                    println!("✅ [P2P] Synchronisation de rattrapage réussie !");
                                    
                                    // 🧹 NETTOYAGE ICI AUSSI !
                                    let mut mp = mempool_clone.lock().unwrap();
                                    mp.retain(|tx| {
                                        !blocks.iter().any(|b| b.transactions.iter().any(|mined_tx| mined_tx.kyber_capsule == tx.kyber_capsule))
                                    });
                                }
                            }
                        } else if process_block {
                            println!("\n🌍 [P2P] Nouveau BLOC {} reçu en direct !", block.header.index);
                            let block_to_clean = block.clone(); 

                            // 💡 NOUVEAU FIX : On enferme le Mutex dans un bloc !
                            // Il mourra automatiquement à l'accolade fermante.
                            let reject_info = {
                                let mut chain = blockchain_clone.lock().unwrap();
                                
                                if let Err(e) = chain.validate_and_add_external_block(block.clone()) {
                                    println!("   🚨 BLOC REJETÉ : {}", e);
                                    let my_genesis = chain.chain[0].header.hash.clone();
                                    let my_height = chain.chain.len() as u64;
                                    // On retourne les infos pour envoyer l'erreur
                                    Some((my_genesis, my_height))
                                } else {
                                    // Pas d'erreur, le bloc est validé !
                                    None
                                }
                            }; // 🔓 Le MutexGuard `chain` est GARANTI détruit ici !

                            // 🌐 Maintenant, on est libre de faire du réseau asynchrone (.await)
                            if let Some((my_genesis, my_height)) = reject_info {
                                // Le bloc a été rejeté, on prévient le mineur
                                let envelope = P2PMessage::Handshake { 
                                    genesis_hash: my_genesis, 
                                    current_height: my_height, 
                                    sender_port: my_port_clone.clone() 
                                };
                                let _ = socket.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
                                
                            } else {
                                // Le bloc a été validé ! On nettoie et on propage.
                                let mut mp = mempool_clone.lock().unwrap();
                                let initial_len = mp.len();
                                mp.retain(|tx| {
                                    !block_to_clean.transactions.iter().any(|mined_tx| mined_tx.kyber_capsule == tx.kyber_capsule)
                                });
                                let removed = initial_len - mp.len();
                                println!("🧹 [MEMPOOL] Nettoyé suite au bloc d'un pair. TX retirées : {}, restantes : {}", removed, mp.len());
                                
                                // 📢 LE RELAIS PROPAGE LE BLOC À TOUS LES AUTRES MINEURS !
                                let peers_list = peers_clone.lock().unwrap().clone();
                                for peer in peers_list {
                                    if peer != full_addr {
                                        let target_clone = peer.clone();
                                        let block_clone = block_to_clean.clone();
                                        let my_port_clone = my_port_clone.clone(); 
                                        let p2p_chain_broadcast = Arc::clone(&blockchain_clone); 
                                        
                                        tokio::spawn(async move {
                                            crate::network::broadcast_block(&target_clone, &my_port_clone, block_clone, p2p_chain_broadcast).await;
                                        });
                                    }
                                }
                            }
                        } else {
                            // 💡 NOUVEAU FIX : Le bloc est dans le passé ! Le mineur est sur une vieille branche (fork).
                            // On lui envoie un Handshake en pleine tête pour le forcer à se mettre à jour !
                            println!("🕰️ [P2P] Bloc obsolète reçu ({}). Le nœud {} est sur un fork, on le réveille !", block.header.index, sender_port);
                            
                            let (my_genesis, my_height) = {
                                let chain_lock = blockchain_clone.lock().unwrap();
                                (chain_lock.chain[0].header.hash.clone(), chain_lock.chain.len() as u64)
                            };

                            let envelope = P2PMessage::Handshake { 
                                genesis_hash: my_genesis, 
                                current_height: my_height, 
                                sender_port: my_port_clone.clone() 
                            };
                            let _ = socket.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
                        }
                    },

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

                    P2PMessage::BroadcastTransaction { tx } => {
                        if tx.is_valid() {
                            let mut pool = mempool_clone.lock().unwrap();
                            if !pool.iter().any(|t| t.dilithium_signature == tx.dilithium_signature) {
                                println!("📥 [MEMPOOL] Nouvelle transaction publique reçue du réseau !");
                                pool.push(tx.clone());
                            }
                        }
                    },
                    
                    // 💡 NOUVEAU : Le serveur donne son Mempool quand on lui demande !
                    P2PMessage::GetMempool => {
                        let pool = mempool_clone.lock().unwrap().clone();
                        let envelope = P2PMessage::MempoolSync { txs: pool };
                        let _ = socket.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;
                    },
                    
                    // On ignore passivement si on le reçoit sans l'avoir demandé
                    P2PMessage::MempoolSync { .. } => {},
					
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
    
    match TcpStream::connect(&address).await {
        Ok(mut stream) => {
            let envelope = P2PMessage::NewBlock { block, sender_port: my_port.to_string() };
            let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;

            // On écoute la réponse du serveur
            if let Some(P2PMessage::Handshake { current_height, .. }) = read_p2p_message(&mut stream).await {
                
                let (my_height, my_genesis) = {
                    let chain = blockchain.lock().unwrap();
                    (chain.chain.len() as u64, chain.chain[0].header.hash.clone())
                };

                if current_height > my_height {
                    println!("⏩ [P2P] Oups, mon bloc a été refusé car je suis en retard ! (Réseau: {}, Moi: {})", current_height, my_height);
                    
                    // 💡 FIX : Le serveur a raccroché. On le RAPPELLE pour aspirer sa chaîne !
                    let target_clone = target_peer.to_string();
                    let my_port_clone = my_port.to_string();
                    let chain_clone = Arc::clone(&blockchain);
                    
                    tokio::spawn(async move {
                        // send_handshake va se connecter, demander la chaîne, la télécharger et faire le resolve_fork !
                        crate::network::send_handshake(&target_clone, &my_port_clone, my_genesis, my_height, chain_clone).await;
                    });

                } else if current_height < my_height {
                    println!("📡 [NAT] Le serveur est en retard (Hauteur: {}). Envoi de ma blockchain complète...", current_height);
                    let blocks_to_send = {
                        let chain = blockchain.lock().unwrap();
                        chain.chain.clone()
                    }; 
                    
                    // 💡 Pareil ici, on ouvre une nouvelle connexion pour lui pousser les blocs
                    let target_clone = target_peer.to_string();
                    tokio::spawn(async move {
                        let addr = format_peer_address(&target_clone);
                        if let Ok(mut new_stream) = TcpStream::connect(&addr).await {
                            let envelope_sync = P2PMessage::SyncResponse { blocks: blocks_to_send };
                            let _ = new_stream.write_all(serde_json::to_string(&envelope_sync).unwrap().as_bytes()).await;
                        }
                    });
                }
            }
        },
        Err(e) => {
            println!("⚠️ [P2P] Impossible de joindre {} pour propager le bloc (Pare-feu ou Nœud hors ligne) : {}", address, e);
        }
    }
}

pub async fn send_handshake(target_peer: &str, my_port: &str, genesis_hash: String, current_height: u64, blockchain: Arc<Mutex<Blockchain>>) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::Handshake { genesis_hash, current_height, sender_port: my_port.to_string() };
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;

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

// 💡 MODIFICATION : On ajoute l'accès à la blockchain pour vérifier le registre noir
pub async fn pull_mempool(target_peer: &str, mempool: Arc<Mutex<Vec<Transaction>>>, blockchain: Arc<Mutex<Blockchain>>) {
    let address = format_peer_address(target_peer);
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        
        let envelope = P2PMessage::GetMempool;
        let _ = stream.write_all(serde_json::to_string(&envelope).unwrap().as_bytes()).await;

        if let Some(P2PMessage::MempoolSync { txs }) = read_p2p_message(&mut stream).await {
            let mut local_mp = mempool.lock().unwrap();
            let chain = blockchain.lock().unwrap(); // 🔒 On regarde le registre de la blockchain
            
            let mut added = 0;
            
            for tx in txs {
                // 💡 FIX : On ne télécharge PAS si elle est déjà dans le mempool OU déjà dépensée !
                if !local_mp.iter().any(|t| t.kyber_capsule == tx.kyber_capsule) 
                   && !chain.spent_key_images.contains(&tx.kyber_capsule) 
                {
                    local_mp.push(tx);
                    added += 1;
                }
            }
            if added > 0 {
                println!("📥 [PULL] {} transaction(s) aspirée(s) depuis le Relais !", added);
            }
        }
    }
}