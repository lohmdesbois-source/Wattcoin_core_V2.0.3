use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use sha2::{Sha256, Digest};

// G est notre point de base pour le facteur d'aveuglement (Blinding factor)
const G: RistrettoPoint = RISTRETTO_BASEPOINT_POINT;

// H est notre point de base pour la valeur (Amount). 
// On le génère de façon transparente ("Nothing up my sleeve") en hachant G.
pub fn get_h_point() -> RistrettoPoint {
    let mut hasher = Sha256::new();
    hasher.update(G.compress().as_bytes());
    let hash = hasher.finalize();
    RistrettoPoint::from_uniform_bytes(hash.as_slice().try_into().unwrap())
}

/// Génère un engagement de Pedersen : C = rG + vH
/// r = blinding_factor (secret)
/// v = amount (valeur)
pub fn commit(amount: u64, blinding_factor: Scalar) -> RistrettoPoint {
    let h = get_h_point();
    let v_scalar = Scalar::from(amount);
    
    // C = (r * G) + (v * H)
    (blinding_factor * G) + (v_scalar * h)
}