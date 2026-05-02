use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use rand::Rng;
use crate::block::Block;
use crate::blockchain::Blockchain;
use crate::transaction::Transaction;
use crate::api::{Order, SharedPool};

// 💡 LE CŒUR DU NOUVEAU RÉSEAU : Le registre des tunnels ouverts
pub type ActivePeers = Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>;

#[derive(Serialize, Deserialize, Debug)]
pub enum P2PMessage {
    Handshake { genesis_hash: String, current_height: u64, sender_port: String },
    SyncRequest { current_height: u64, last_hash: String, sender_port: String }, 
    SyncResponse { blocks: Vec<Block> },
    NewBlock { block: Block, sender_port: String }, 
    WhisperTransaction { tx: Transaction },    
    BroadcastTransaction { tx: Transaction },  
    BroadcastOrder { order: Order },
    GetMempool,
    MempoolSync { txs: Vec<Transaction> },
}

// 💡 Lecture en continu (NDJSON). On lit ligne par ligne jusqu'au \n.
async fn read_p2p_message(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Option<P2PMessage> {
    let mut line = String::new();
    match reader.read_line(&mut line).await {
        Ok(0) => None, // La connexion a été coupée
        Ok(_) => {
            serde_json::from_str::<P2PMessage>(&line.trim()).ok()
        }
        Err(_) => None,
    }
}

// 💡 Fonction utilitaire pour envoyer un message avec un \n final
async fn send_message_to_channel(sender: &mpsc::Sender<String>, message: P2PMessage) {
    let mut json_str = serde_json::to_string(&message).unwrap();
    json_str.push('\n'); // Crucial pour le BufReader !
    let _ = sender.send(json_str).await;
}

pub async fn start_p2p_server(host_ip: &str, port: &str, blockchain: Arc<Mutex<Blockchain>>, mempool: Arc<Mutex<Vec<Transaction>>>, dex_pool: SharedPool, known_peers: crate::SharedPeers, active_peers: ActivePeers) {
    let address = format!("{}:{}", host_ip, port);
    let listener = TcpListener::bind(&address).await.unwrap();
    println!("📡 Serveur P2P (Tunnels Persistants) à l'écoute sur TCP/{}...", port);
    
    let my_port = port.to_string(); 

    loop {
        let (socket, peer_addr) = listener.accept().await.unwrap();
        let peer_ip = peer_addr.ip().to_string();
        
        start_peer_connection(
            socket, peer_ip, my_port.clone(), 
            Arc::clone(&blockchain), Arc::clone(&mempool), Arc::clone(&dex_pool), 
            Arc::clone(&known_peers), Arc::clone(&active_peers)
        );
    }
}

// 🧠 LE GESTIONNAIRE DE TUNNEL
fn start_peer_connection(
    socket: TcpStream, peer_ip: String, my_port: String,
    blockchain: Arc<Mutex<Blockchain>>, mempool: Arc<Mutex<Vec<Transaction>>>, dex_pool: SharedPool,
    known_peers: crate::SharedPeers, active_peers: ActivePeers
) {
    let (read_half, mut write_half) = socket.into_split();
    let mut reader = BufReader::new(read_half);
    let (tx, mut rx) = mpsc::channel::<String>(100);

    let temp_peer_id = format!("{}:incoming", peer_ip);
    active_peers.lock().unwrap().insert(temp_peer_id.clone(), tx.clone());

    // 📤 TÂCHE D'ÉCRITURE
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write_half.write_all(msg.as_bytes()).await.is_err() { break; }
        }
    });

    // 📥 TÂCHE DE LECTURE
    tokio::spawn(async move {
        let mut actual_peer_id = temp_peer_id.clone();

        while let Some(message) = read_p2p_message(&mut reader).await {
            match message {
                P2PMessage::Handshake { genesis_hash, current_height, sender_port } => {
                    actual_peer_id = format!("{}:{}", peer_ip, sender_port);
                    known_peers.lock().unwrap().insert(actual_peer_id.clone());
                    
                    // 💡 FIX : Le bloc {} garantit la mort du MutexGuard
                    {
                        let mut ap = active_peers.lock().unwrap();
                        if let Some(sender) = ap.remove(&temp_peer_id) {
                            ap.insert(actual_peer_id.clone(), sender);
                        }
                    } // 🔓 Le cadenas est détruit à 100% ici

                    let (is_behind, i_am_ahead, my_height, my_hash, genesis_valid) = {
                        let chain = blockchain.lock().unwrap(); 
                        let my_h = chain.chain.len() as u64;
                        (
                            current_height > my_h, 
                            my_h > current_height, 
                            my_h, 
                            chain.chain.last().unwrap().header.hash.clone(),
                            genesis_hash == chain.chain[0].header.hash
                        )
                    }; 

                    if !genesis_valid {
                        println!("🚨 [P2P] INTRUSION REJETÉE.");
                        break; 
                    }

                    if is_behind {
                        println!("🔄 [P2P] Je suis en retard. Demande du delta à {}...", sender_port);
                        send_message_to_channel(&tx, P2PMessage::SyncRequest { current_height: my_height, last_hash: my_hash, sender_port: my_port.clone() }).await;
                    } else if i_am_ahead {
                        send_message_to_channel(&tx, P2PMessage::Handshake { genesis_hash, current_height: my_height, sender_port: my_port.clone() }).await;
                    }
                },

                P2PMessage::SyncRequest { current_height, last_hash, sender_port } => {
                    let blocks_to_send = {
                        let chain = blockchain.lock().unwrap(); 
                        let my_height = chain.chain.len() as u64;

                        if my_height > current_height {
                            println!("📤 [P2P] Calcul du Delta pour {}...", sender_port);
                            let mut start_idx = current_height as usize;
                            let check_idx = start_idx.saturating_sub(1); 

                            if check_idx < chain.chain.len() && chain.chain[check_idx].header.hash == last_hash {
                                Some(chain.chain[start_idx..].to_vec())
                            } else {
                                println!("🔀 Fork détecté avec {} ! On recule pour la greffe.", sender_port);
                                start_idx = start_idx.saturating_sub(10);
                                if start_idx == 0 { start_idx = 1; } 
                                Some(chain.chain[start_idx..].to_vec())
                            }
                        } else { None }
                    }; 

                    if let Some(blocks) = blocks_to_send {
                        send_message_to_channel(&tx, P2PMessage::SyncResponse { blocks }).await;
                    }
                },
                
                P2PMessage::SyncResponse { blocks } => {
                    if blocks.is_empty() { continue; }
                    let mut chain = blockchain.lock().unwrap(); 
                    println!("📦 [P2P] Réception d'un Delta ({} blocs) !", blocks.len());
                    
                    if chain.resolve_partial_fork(blocks.clone()) { 
                        println!("✅ Synchronisation partielle réussie ! Nous sommes à jour.");
                        let mut mp = mempool.lock().unwrap();
                        mp.retain(|tx| { !blocks.iter().any(|b| b.transactions.iter().any(|mined_tx| mined_tx.kyber_capsule == tx.kyber_capsule)) });
                    }
                },

                P2PMessage::NewBlock { block, sender_port } => {
                    let reject_info = {
                        let mut chain = blockchain.lock().unwrap();
                        if let Err(e) = chain.validate_and_add_external_block(block.clone()) {
                            println!("   🚨 BLOC REJETÉ : {}", e);
                            Some((chain.chain[0].header.hash.clone(), chain.chain.len() as u64))
                        } else { None }
                    };

                    if let Some((my_genesis, my_height)) = reject_info {
                        println!("🕰️ [P2P] Bloc obsolète. On réveille {} !", sender_port);
                        send_message_to_channel(&tx, P2PMessage::Handshake { genesis_hash: my_genesis, current_height: my_height, sender_port: my_port.clone() }).await;
                    } else {
                        println!("\n====================================================================");
						println!("🌍 [RÉSEAU] NOUVEAU BLOC {} REÇU ! (Source: {})", block.header.index, sender_port);
						println!("🔗 Hash: {}", block.header.hash);
						println!("====================================================================");
						println!("✅ Bloc {} validé et ajouté à la chaîne locale.", block.header.index);
                        mempool.lock().unwrap().retain(|t| { !block.transactions.iter().any(|mined_tx| mined_tx.kyber_capsule == t.kyber_capsule) });
                        
                        // 📢 On propage ce bloc à TOUS nos autres tunnels !
                        let env = P2PMessage::NewBlock { block: block.clone(), sender_port: my_port.clone() };
                        let mut json_str = serde_json::to_string(&env).unwrap();
                        json_str.push('\n');
                        
                        let ap = active_peers.lock().unwrap().clone();
                        for (peer_id, sender) in ap.iter() {
                            if peer_id != &actual_peer_id {
                                let _ = sender.try_send(json_str.clone());
                            }
                        }
                    }
                },

                P2PMessage::WhisperTransaction { tx: in_tx } => {
                    let mut rng = rand::thread_rng();
                    if rng.gen_range(1..=10) <= 2 {
                        println!("🌼 [DANDELION] Explosion du pissenlit ! Diffusion publique.");
                        mempool.lock().unwrap().push(in_tx);
                    } else {
                        println!("🤫 [DANDELION] Relais furtif de la TX...");
                    }
                },

                P2PMessage::BroadcastTransaction { tx: in_tx } => {
                    if in_tx.is_valid() {
                        let mut pool = mempool.lock().unwrap();
                        if !pool.iter().any(|t| t.dilithium_signature == in_tx.dilithium_signature) {
                            println!("📥 [MEMPOOL] Nouvelle transaction publique reçue !");
                            pool.push(in_tx);
                        }
                    }
                },

                P2PMessage::GetMempool => {
                    let pool = mempool.lock().unwrap().clone();
                    send_message_to_channel(&tx, P2PMessage::MempoolSync { txs: pool }).await;
                },

                P2PMessage::MempoolSync { txs } => {
                    let mut local_mp = mempool.lock().unwrap();
                    let chain = blockchain.lock().unwrap(); 
                    let mut added = 0;
                    for t in txs {
                        if !local_mp.iter().any(|x| x.kyber_capsule == t.kyber_capsule) && !chain.spent_key_images.contains(&t.kyber_capsule) {
                            local_mp.push(t);
                            added += 1;
                        }
                    }
                    if added > 0 { println!("📥 [PULL] {} transaction(s) aspirée(s) !", added); }
                },

                P2PMessage::BroadcastOrder { order } => {
                    let mut pool = dex_pool.lock().unwrap();
                    if !pool.iter().any(|o| o.id == order.id) {
                        println!("🌊 [P2P DEX] Ordre reçu du réseau : {} {} WATT", order.order_type, order.amount_flames);
                        pool.push(order);
                    }
                },
            }
        }
        
        println!("🔌 [P2P] Connexion perdue avec {}.", actual_peer_id);
        active_peers.lock().unwrap().remove(&actual_peer_id);
    });
}

// --- FONCTIONS RÉSEAU POUR LE MINEUR ---

// Ouvre le tunnel au démarrage du mineur
pub async fn connect_to_network(target_peer: &str, my_port: &str, blockchain: Arc<Mutex<Blockchain>>, mempool: Arc<Mutex<Vec<Transaction>>>, dex_pool: SharedPool, known_peers: crate::SharedPeers, active_peers: ActivePeers) {
    let address = if target_peer.contains(':') { target_peer.to_string() } else { format!("127.0.0.1:{}", target_peer) };
    
    if let Ok(socket) = TcpStream::connect(&address).await {
        println!("🔗 [P2P] Tunnel connecté au réseau via {} !", address);
        
        let (my_genesis, my_height) = {
            let chain = blockchain.lock().unwrap();
            (chain.chain[0].header.hash.clone(), chain.chain.len() as u64)
        };

        start_peer_connection(
            socket, address.clone(), my_port.to_string(), 
            Arc::clone(&blockchain), Arc::clone(&mempool), Arc::clone(&dex_pool), 
            Arc::clone(&known_peers), Arc::clone(&active_peers)
        );

        // 💡 FIX : On clone le sender dans un bloc isolé
        let ip_only = address.split(':').next().unwrap_or(&address).to_string();
		let sender_opt = {
			active_peers.lock().unwrap().get(&format!("{}:incoming", ip_only)).cloned()
		};
        // 🔓 Le Mutex est fermé. On peut utiliser le sender cloné en mode asynchrone !
        if let Some(sender) = sender_opt {
            send_message_to_channel(&sender, P2PMessage::Handshake { 
                genesis_hash: my_genesis, 
                current_height: my_height, 
                sender_port: my_port.to_string() 
            }).await;
        }
    } else {
        println!("⚠️ [P2P] Impossible de joindre le nœud de bootstrap {}.", address);
    }
}

// Diffuse le bloc dans TOUS les tunnels ouverts instantanément
pub async fn broadcast_mined_block(my_port: &str, block: Block, active_peers: ActivePeers) {
    let envelope = P2PMessage::NewBlock { block, sender_port: my_port.to_string() };
    let mut json_str = serde_json::to_string(&envelope).unwrap();
    json_str.push('\n');

    let peers = active_peers.lock().unwrap().clone();
    for (peer_id, sender) in peers.iter() {
        println!("🚀 Envoi du bloc dans le tunnel vers {}...", peer_id);
        let _ = sender.try_send(json_str.clone());
    }
}

// Diffuse une demande de Mempool dans TOUS les tunnels
pub async fn request_mempool(active_peers: ActivePeers) {
    let envelope = P2PMessage::GetMempool;
    let mut json_str = serde_json::to_string(&envelope).unwrap();
    json_str.push('\n');

    let peers = active_peers.lock().unwrap().clone();
    for (_, sender) in peers.iter() {
        let _ = sender.try_send(json_str.clone());
    }
}

pub async fn broadcast_transaction(target_peer: &str, tx: Transaction) {
    let address = if target_peer.contains(':') { target_peer.to_string() } else { format!("127.0.0.1:{}", target_peer) };
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::BroadcastTransaction { tx };
        let mut json_str = serde_json::to_string(&envelope).unwrap();
        json_str.push('\n'); // 💡 Indispensable pour le BufReader !
        let _ = stream.write_all(json_str.as_bytes()).await;
    }
}

pub async fn broadcast_order(target_peer: &str, order: Order) {
    let address = if target_peer.contains(':') { target_peer.to_string() } else { format!("127.0.0.1:{}", target_peer) };
    if let Ok(mut stream) = TcpStream::connect(&address).await {
        let envelope = P2PMessage::BroadcastOrder { order };
        let mut json_str = serde_json::to_string(&envelope).unwrap();
        json_str.push('\n');
        let _ = stream.write_all(json_str.as_bytes()).await;
    }
}