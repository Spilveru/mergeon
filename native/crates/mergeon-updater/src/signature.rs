//! Verificación de firma Ed25519 sobre los bytes exactos de `latest*.yml`.
//!
//! Cada manifiesto publicado se acompaña de un archivo separado `<nombre>.sig`
//! que contiene la firma Ed25519 (64 bytes en base64) calculada sobre los bytes
//! crudos del manifiesto. La clave pública (32 bytes en base64) se embebe al
//! compilar mediante `MERGEON_UPDATE_PUBLIC_KEY`.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as BASE64, STANDARD_NO_PAD as BASE64_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

use crate::error::UpdaterError;

/// Clave pública Ed25519 (32 bytes en base64) fijada al compilar.
pub const BUILT_IN_UPDATE_PUBLIC_KEY: Option<&str> = option_env!("MERGEON_UPDATE_PUBLIC_KEY");

/// Extensión del archivo de firma separado junto a cada `latest*.yml`.
pub const SIGNATURE_SUFFIX: &str = ".sig";

/// Tamaño máximo aceptado para un archivo `.sig` (64 bytes en base64 son ~88 B).
pub const MAX_SIGNATURE_BYTES: u64 = 1024;

/// Nombre del archivo de firma para un manifiesto (`latest.yml` → `latest.yml.sig`).
pub fn signature_file_name(manifest_file_name: &str) -> String {
    format!("{manifest_file_name}{SIGNATURE_SUFFIX}")
}

fn decode_base64_exact(text: &str, expected_len: usize, label: &str) -> Result<Vec<u8>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} is empty"));
    }
    let bytes = BASE64
        .decode(trimmed)
        .or_else(|_| BASE64_NO_PAD.decode(trimmed.trim_end_matches('=')))
        .map_err(|error| format!("{label} is not valid base64: {error}"))?;
    if bytes.len() != expected_len {
        return Err(format!(
            "{label} must be {expected_len} bytes, got {}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Verifica que `signature_b64` sea una firma Ed25519 válida de `manifest_bytes`
/// bajo `public_key_b64`.
pub fn verify_manifest_signature(
    manifest_bytes: &[u8],
    signature_b64: &str,
    public_key_b64: &str,
) -> Result<(), UpdaterError> {
    let public_key = decode_base64_exact(public_key_b64, 32, "update public key")
        .map_err(|detail| UpdaterError::InvalidSignature { detail })?;
    let signature = decode_base64_exact(signature_b64, 64, "manifest signature")
        .map_err(|detail| UpdaterError::InvalidSignature { detail })?;

    UnparsedPublicKey::new(&ED25519, &public_key)
        .verify(manifest_bytes, &signature)
        .map_err(|_| UpdaterError::InvalidSignature {
            detail: "Ed25519 manifest signature verification failed".to_string(),
        })
}

/// Firma `manifest_bytes` con una clave privada Ed25519 en base64 (documento
/// PKCS#8 o semilla cruda de 32 bytes) y devuelve la firma de 64 bytes en base64.
pub fn sign_manifest_bytes(manifest_bytes: &[u8], private_key_b64: &str) -> Result<String, String> {
    let trimmed = private_key_b64.trim();
    let bytes = BASE64
        .decode(trimmed)
        .or_else(|_| BASE64_NO_PAD.decode(trimmed.trim_end_matches('=')))
        .map_err(|error| format!("private key is not valid base64: {error}"))?;

    let keypair = if bytes.len() == 32 {
        Ed25519KeyPair::from_seed_unchecked(&bytes)
            .map_err(|error| format!("invalid Ed25519 seed: {error}"))?
    } else {
        Ed25519KeyPair::from_pkcs8(&bytes)
            .map_err(|error| format!("invalid Ed25519 PKCS#8 key: {error}"))?
    };

    Ok(BASE64.encode(keypair.sign(manifest_bytes).as_ref()))
}

/// Genera un par de claves Ed25519 `(private_pkcs8_b64, public_key_b64)`.
pub fn generate_signing_keypair() -> Result<(String, String), String> {
    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|error| format!("failed to generate Ed25519 keypair: {error}"))?;
    let keypair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|error| format!("failed to parse generated Ed25519 keypair: {error}"))?;
    Ok((
        BASE64.encode(pkcs8.as_ref()),
        BASE64.encode(keypair.public_key().as_ref()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_and_verifies_manifest_bytes() {
        let (private_key, public_key) = generate_signing_keypair().unwrap();
        let manifest = b"version: 0.14.0\npath: Mergeon.AppImage\n";

        let signature = sign_manifest_bytes(manifest, &private_key).unwrap();
        verify_manifest_signature(manifest, &signature, &public_key).unwrap();
        // Tolera saltos de línea alrededor de la firma en el archivo `.sig`.
        verify_manifest_signature(manifest, &format!("  {signature}\n"), &public_key).unwrap();
    }

    #[test]
    fn rejects_tampered_manifest_wrong_key_and_malformed_inputs() {
        let (private_key, public_key) = generate_signing_keypair().unwrap();
        let (_, other_public_key) = generate_signing_keypair().unwrap();
        let manifest = b"version: 0.14.0\npath: Mergeon.AppImage\n";
        let signature = sign_manifest_bytes(manifest, &private_key).unwrap();

        let tampered = b"version: 0.14.1\npath: Mergeon.AppImage\n";
        assert!(matches!(
            verify_manifest_signature(tampered, &signature, &public_key),
            Err(UpdaterError::InvalidSignature { .. })
        ));
        assert!(matches!(
            verify_manifest_signature(manifest, &signature, &other_public_key),
            Err(UpdaterError::InvalidSignature { .. })
        ));
        assert!(matches!(
            verify_manifest_signature(manifest, "", &public_key),
            Err(UpdaterError::InvalidSignature { .. })
        ));
        assert!(matches!(
            verify_manifest_signature(manifest, "not-base64!", &public_key),
            Err(UpdaterError::InvalidSignature { .. })
        ));
        assert!(matches!(
            verify_manifest_signature(manifest, &signature, "c2hvcnQ="),
            Err(UpdaterError::InvalidSignature { .. })
        ));
    }

    #[test]
    fn supports_signing_with_a_raw_32_byte_seed() {
        let seed = [42u8; 32];
        let seed_b64 = BASE64.encode(seed);
        let keypair = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let public_key = BASE64.encode(keypair.public_key().as_ref());

        let sig = sign_manifest_bytes(b"manifest", &seed_b64).unwrap();
        verify_manifest_signature(b"manifest", &sig, &public_key).unwrap();
    }
}
