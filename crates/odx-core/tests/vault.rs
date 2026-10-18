use libsodium_rs::{crypto_aead::xchacha20poly1305 as aead, ensure_init};

#[test]
fn xchacha20poly1305_matches_libsodium_reference_vector() {
    ensure_init().unwrap();
    let key_bytes: [u8; 32] = std::array::from_fn(|i| 0x80 + i as u8);
    let key = aead::Key::from_bytes(&key_bytes).unwrap();
    let nonce_bytes = [
        0x07, 0x00, 0x00, 0x00, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a,
        0x4b, 0x4c, 0x4d, 0x4e, 0x4f, 0x50, 0x51, 0x52, 0x53,
    ];
    let nonce = aead::Nonce::from_bytes(nonce_bytes);
    let aad = [
        0x50, 0x51, 0x52, 0x53, 0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7,
    ];
    let message = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let ciphertext = aead::encrypt(message, Some(&aad), &nonce, &key).unwrap();
    assert_eq!(
        hex::encode(&ciphertext),
        "f8ebea4875044066fc162a0604e171feecfb3d20425248563bcfd5a155dcc47bbda70b86e5ab9b55002bd1274c02db35321acd7af8b2e2d25015e136b7679458e9f43243bf719d639badb5feac03f80a19a96ef10cb1d15333a837b90946ba3854ee74da3f2585efc7e1e170e17e15e563e77601f4f85cafa8e5877614e143e68420"
    );
    assert_eq!(
        aead::decrypt(&ciphertext, Some(&aad), &nonce, &key).unwrap(),
        message
    );
}
