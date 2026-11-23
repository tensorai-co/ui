//! ECDSA P-256 signature checks with the system Security framework.

use std::ffi::c_void;

use crate::backend::macos_objc::{Id, class, msg};
use crate::{Error, Result};

type CFTypeRef = *const c_void;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecAttrKeyType: CFTypeRef;
    static kSecAttrKeyTypeECSECPrimeRandom: CFTypeRef;
    static kSecAttrKeyClass: CFTypeRef;
    static kSecAttrKeyClassPublic: CFTypeRef;
    static kSecKeyAlgorithmECDSASignatureMessageX962SHA256: CFTypeRef;
    fn SecKeyCreateWithData(
        data: CFTypeRef,
        attributes: CFTypeRef,
        error: *mut CFTypeRef,
    ) -> CFTypeRef;
    fn SecKeyVerifySignature(
        key: CFTypeRef,
        algorithm: CFTypeRef,
        signed_data: CFTypeRef,
        signature: CFTypeRef,
        error: *mut CFTypeRef,
    ) -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
}

fn ns_data(bytes: &[u8]) -> Id {
    msg!(Id; class("NSData"), "dataWithBytes:length:",
        bytes.as_ptr() as *const c_void => *const c_void, bytes.len() => usize)
}

/// Verifies a DER (X9.62) ECDSA-SHA256 `signature` of `message` with a P-256
/// public `key` given as an uncompressed point (65 bytes, `0x04 ‖ X ‖ Y`).
pub(crate) fn verify_p256(key: &[u8], message: &[u8], signature: &[u8]) -> Result<bool> {
    if key.len() != 65 || key[0] != 0x04 {
        return Err(Error::InvalidState(
            "update key must be an uncompressed P-256 public key (65 bytes)".into(),
        ));
    }
    // SAFETY: Security framework constants are valid CFStrings for the process lifetime.
    let (k_type, k_ec, k_class, k_public, alg) = unsafe {
        (
            kSecAttrKeyType,
            kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeyClass,
            kSecAttrKeyClassPublic,
            kSecKeyAlgorithmECDSASignatureMessageX962SHA256,
        )
    };
    let attrs = msg!(Id; class("NSMutableDictionary"), "dictionary");
    msg!((); attrs, "setObject:forKey:", k_ec as Id => Id, k_type as Id => Id);
    msg!((); attrs, "setObject:forKey:", k_public as Id => Id, k_class as Id => Id);
    let mut err: CFTypeRef = std::ptr::null();
    // SAFETY: NSData / NSDictionary are toll-free bridged to CFData / CFDictionary.
    let sec_key =
        unsafe { SecKeyCreateWithData(ns_data(key) as CFTypeRef, attrs as CFTypeRef, &mut err) };
    if sec_key.is_null() {
        if !err.is_null() {
            // SAFETY: `err` is a +1 CFError we own.
            unsafe { CFRelease(err) };
        }
        return Err(Error::InvalidState(
            "update key isn't a valid P-256 public key".into(),
        ));
    }
    let mut err: CFTypeRef = std::ptr::null();
    // SAFETY: all arguments are valid CF objects; `sec_key` is +1 and released below.
    let ok = unsafe {
        let ok = SecKeyVerifySignature(
            sec_key,
            alg,
            ns_data(message) as CFTypeRef,
            ns_data(signature) as CFTypeRef,
            &mut err,
        );
        if !err.is_null() {
            CFRelease(err);
        }
        CFRelease(sec_key);
        ok
    };
    Ok(ok != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Signs with the system `openssl`, the way `ui-pack update-feed` does.
    #[test]
    fn verifies_openssl_signatures_and_rejects_tampering() {
        let dir = std::env::temp_dir().join(format!("ui-sec-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("k.pem");
        let ok = |args: &[&str]| {
            Command::new("openssl")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap()
        };
        ok(&[
            "ecparam",
            "-name",
            "prime256v1",
            "-genkey",
            "-noout",
            "-out",
            "k.pem",
        ]);
        let spki = ok(&[
            "ec",
            "-in",
            "k.pem",
            "-pubout",
            "-outform",
            "DER",
            "-conv_form",
            "uncompressed",
        ])
        .stdout;
        assert!(spki.len() >= 65, "openssl produced a public key");
        let point = &spki[spki.len() - 65..];
        std::fs::write(dir.join("m.txt"), b"{\"version\":\"9.9.9\"}").unwrap();
        let sig = ok(&["dgst", "-sha256", "-sign", "k.pem", "m.txt"]).stdout;

        assert!(verify_p256(point, b"{\"version\":\"9.9.9\"}", &sig).unwrap());
        assert!(!verify_p256(point, b"{\"version\":\"9.9.8\"}", &sig).unwrap());
        assert!(verify_p256(&point[1..], b"x", &sig).is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = key;
    }
}
