use std::io::{self, Read};

use minisign_verify::{Error as MinisignError, PublicKey, Signature};

use super::version::Version;

pub const MAX_SIGNATURE_BYTES: u64 = 4 * 1024;
pub const MAX_ZIP_BYTES: u64 = 256 * 1024 * 1024;
#[cfg(target_arch = "x86_64")]
pub const PLATFORM: &str = "windows-x64";
#[cfg(target_arch = "aarch64")]
pub const PLATFORM: &str = "windows-arm64";
const PRODUCT: &str = "zenkai";
const READ_CHUNK: usize = 64 * 1024;

// The maintainer's real public keys, current then next (docs/RELEASING.md).
#[cfg(any(test, not(feature = "test-keys")))]
const RELEASE_KEYS: &[&str] = &[
    "RWRzxvrIpXm8tYQYJqpsts0gCaS26UJy/kZssupbEyydweKE23HDAYjN",
    "RWTBj4N8d3kF/6HqBJ4Z7EOdhPzhQ5diqM+4vshHyN/HFoXy3IYvBsR1",
];
#[cfg(not(any(test, feature = "test-keys")))]
const TRUSTED_KEYS: &[&str] = RELEASE_KEYS;
// TEST keys, current and next: their secret seeds are in tests/fixtures/update/make-fixtures.mjs,
// so they must never be trusted by a build that ships.
#[cfg(any(test, feature = "test-keys"))]
const TRUSTED_KEYS: &[&str] = &[
    "RWR2xaKuXEhnqKoTmGepuFVdNFslHlo2RK/tTpOEQP0gPcf3OBdsKMGO",
    "RWTLQ9vx1ajpnR7Ateo1t/yue3+jrifpcJftIpn7oSnjdvQLkO3/lTIy",
];

#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    #[error("could not read the zip: {0}")]
    Read(#[source] io::Error),
    #[error("the signature is larger than {} bytes", MAX_SIGNATURE_BYTES)]
    SignatureTooLarge,
    #[error("the zip is larger than {} bytes", MAX_ZIP_BYTES)]
    ZipTooLarge,
    #[error("the signature is not a minisign signature")]
    Malformed,
    #[error("the signature uses the legacy format")]
    Legacy,
    #[error("a key compiled into the app is not a minisign public key")]
    UnusableKey,
    #[error("no trusted key has the key id of the signature")]
    UnknownKey,
    #[error("the signature does not match the zip")]
    Invalid,
    #[error("the trusted comment is not zenkai/<version>/<platform>")]
    Comment,
    #[error("the zip is signed for another platform: {0}")]
    OtherPlatform(String),
    #[error("the zip is signed as {signed}, not as the chosen {chosen}")]
    OtherVersion { signed: Version, chosen: Version },
    #[error("the signed version {0} is not newer than the installed one")]
    NotNewer(Version),
}

#[derive(Debug, PartialEq, Eq)]
pub struct SignedRelease<'a> {
    pub version: Version,
    pub platform: &'a str,
}

// Without a usable key nothing can verify, so the app does not offer to download anything.
pub fn has_trusted_key() -> bool {
    TRUSTED_KEYS
        .iter()
        .any(|key| PublicKey::from_base64(key).is_ok())
}

// Exactly `zenkai/<version>/<platform>`: one more field, a tab or a space and it is not ours.
pub fn parse_comment(comment: &str) -> Option<SignedRelease<'_>> {
    let mut fields = comment.split('/');
    let product = fields.next()?;
    let version = fields.next()?;
    let platform = fields.next()?;
    if fields.next().is_some() || product != PRODUCT || platform.is_empty() {
        return None;
    }
    Some(SignedRelease {
        version: Version::parse_bare(version)?,
        platform,
    })
}

pub fn decode(signature: &[u8]) -> Result<Signature, SignatureError> {
    if signature.len() as u64 > MAX_SIGNATURE_BYTES {
        return Err(SignatureError::SignatureTooLarge);
    }
    let text = std::str::from_utf8(signature).map_err(|_| SignatureError::Malformed)?;
    Signature::decode(text).map_err(|_| SignatureError::Malformed)
}

// Accepts the zip only when a trusted key signed it, the global signature covers the comment,
// and the version and platform read from that verified comment are the ones asked for.
pub fn verify(
    zip: &mut impl Read,
    signature: &[u8],
    chosen: Version,
    installed: Version,
) -> Result<(), SignatureError> {
    verify_with_keys(zip, signature, chosen, installed, TRUSTED_KEYS)
}

fn verify_with_keys(
    zip: &mut impl Read,
    signature: &[u8],
    chosen: Version,
    installed: Version,
    keys: &[&str],
) -> Result<(), SignatureError> {
    let signature = decode(signature)?;
    let keys = keys
        .iter()
        .map(|key| PublicKey::from_base64(key).map_err(|_| SignatureError::UnusableKey))
        .collect::<Result<Vec<_>, _>>()?;
    let mut verifier = None;
    for key in &keys {
        match key.verify_stream(&signature) {
            Ok(found) => {
                verifier = Some(found);
                break;
            }
            Err(MinisignError::UnexpectedKeyId) => {}
            Err(MinisignError::UnsupportedLegacyMode) => return Err(SignatureError::Legacy),
            Err(_) => return Err(SignatureError::Invalid),
        }
    }
    let mut verifier = verifier.ok_or(SignatureError::UnknownKey)?;
    let mut buffer = vec![0; READ_CHUNK];
    let mut read_so_far = 0u64;
    loop {
        let read = match zip.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(SignatureError::Read(error)),
        };
        read_so_far += read as u64;
        if read_so_far > MAX_ZIP_BYTES {
            return Err(SignatureError::ZipTooLarge);
        }
        verifier.update(&buffer[..read]);
    }
    verifier.finalize().map_err(|_| SignatureError::Invalid)?;
    check_comment(signature.trusted_comment(), chosen, installed)
}

fn check_comment(comment: &str, chosen: Version, installed: Version) -> Result<(), SignatureError> {
    let signed = parse_comment(comment).ok_or(SignatureError::Comment)?;
    if signed.platform != PLATFORM {
        return Err(SignatureError::OtherPlatform(signed.platform.to_owned()));
    }
    if signed.version != chosen {
        return Err(SignatureError::OtherVersion {
            signed: signed.version,
            chosen,
        });
    }
    if signed.version <= installed {
        return Err(SignatureError::NotNewer(signed.version));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::version::Version;
    use super::{
        MAX_SIGNATURE_BYTES, PLATFORM, RELEASE_KEYS, SignatureError, SignedRelease, TRUSTED_KEYS,
        decode, has_trusted_key, parse_comment, verify, verify_with_keys,
    };

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "fixtures",
            "update",
            name,
        ]
        .iter()
        .collect();
        std::fs::read(path).unwrap()
    }

    fn zip() -> Vec<u8> {
        fixture("zenkai-v0.2.0-windows-x64.zip")
    }

    fn check(
        zip: &[u8],
        signature: &[u8],
        chosen: &str,
        installed: &str,
    ) -> Result<(), SignatureError> {
        verify(
            &mut &zip[..],
            signature,
            version(chosen),
            version(installed),
        )
    }

    fn check_fixture(signature: &str) -> Result<(), SignatureError> {
        check(&zip(), &fixture(signature), "0.2.0", "0.1.0")
    }

    #[test]
    fn the_test_build_trusts_exactly_the_two_test_keys() {
        assert_eq!(TRUSTED_KEYS.len(), 2);
        for key in TRUSTED_KEYS {
            assert!(
                minisign_verify::PublicKey::from_base64(key).is_ok(),
                "{key}"
            );
        }
    }

    // Empty until the real pair is added; then two distinct usable keys, neither a TEST one.
    #[test]
    fn a_release_build_trusts_only_the_real_pair() {
        assert!(matches!(RELEASE_KEYS.len(), 0 | 2), "{RELEASE_KEYS:?}");
        for key in RELEASE_KEYS {
            assert!(
                minisign_verify::PublicKey::from_base64(key).is_ok(),
                "{key}"
            );
            assert!(!TRUSTED_KEYS.contains(key), "{key} is a TEST key");
        }
        if let [current, next] = RELEASE_KEYS {
            assert_ne!(current, next);
        }
    }

    #[test]
    fn a_test_build_can_verify_updates() {
        assert!(has_trusted_key());
    }

    #[test]
    fn the_fixtures_are_signed_for_this_platform() {
        assert_eq!(PLATFORM, "windows-x64");
    }

    #[test]
    fn a_release_signed_with_the_current_key_verifies() {
        check_fixture("good.minisig").unwrap();
    }

    #[test]
    fn a_release_signed_with_the_next_key_verifies() {
        check_fixture("next-key.minisig").unwrap();
    }

    #[test]
    fn one_altered_byte_anywhere_in_the_zip_is_refused() {
        let original = zip();
        let signature = fixture("good.minisig");
        for position in [0, original.len() / 2, original.len() - 1] {
            let mut altered = original.clone();
            altered[position] ^= 0x01;
            assert!(
                matches!(
                    check(&altered, &signature, "0.2.0", "0.1.0"),
                    Err(SignatureError::Invalid)
                ),
                "{position}"
            );
        }
        let mut longer = original.clone();
        longer.push(0);
        assert!(matches!(
            check(&longer, &signature, "0.2.0", "0.1.0"),
            Err(SignatureError::Invalid)
        ));
    }

    #[test]
    fn an_old_zip_well_signed_is_refused_as_the_new_version() {
        assert!(matches!(
            check_fixture("old-version.minisig"),
            Err(SignatureError::OtherVersion { signed, chosen })
                if signed == version("0.1.0") && chosen == version("0.2.0")
        ));
    }

    #[test]
    fn a_version_not_newer_than_the_installed_one_is_refused() {
        let signature = fixture("good.minisig");
        for installed in ["0.2.0", "0.3.0"] {
            assert!(matches!(
                check(&zip(), &signature, "0.2.0", installed),
                Err(SignatureError::NotNewer(found)) if found == version("0.2.0")
            ));
        }
    }

    #[test]
    fn a_zip_for_another_platform_is_refused() {
        assert!(matches!(
            check_fixture("other-platform.minisig"),
            Err(SignatureError::OtherPlatform(platform)) if platform == "linux-x64"
        ));
    }

    #[test]
    fn a_comment_with_extra_fields_is_refused() {
        assert!(matches!(
            check_fixture("extra-field.minisig"),
            Err(SignatureError::Comment)
        ));
        assert!(matches!(
            check_fixture("default-comment.minisig"),
            Err(SignatureError::Comment)
        ));
    }

    #[test]
    fn a_key_outside_the_trusted_pair_is_refused() {
        assert!(matches!(
            check_fixture("wrong-key.minisig"),
            Err(SignatureError::UnknownKey)
        ));
    }

    #[test]
    fn a_trusted_key_id_on_a_stranger_signature_is_refused() {
        assert!(matches!(
            check_fixture("forged-key-id.minisig"),
            Err(SignatureError::Invalid)
        ));
    }

    #[test]
    fn a_legacy_signature_is_refused() {
        assert!(matches!(
            check_fixture("legacy.minisig"),
            Err(SignatureError::Legacy)
        ));
    }

    #[test]
    fn an_edited_trusted_comment_breaks_the_global_signature() {
        let text = String::from_utf8(fixture("good.minisig")).unwrap();
        let edited = text.replace("zenkai/0.2.0/windows-x64", "zenkai/0.3.0/windows-x64");
        assert_ne!(edited, text);
        assert!(matches!(
            check(&zip(), edited.as_bytes(), "0.3.0", "0.1.0"),
            Err(SignatureError::Invalid)
        ));
    }

    #[test]
    fn without_any_trusted_key_nothing_verifies() {
        let result = verify_with_keys(
            &mut &zip()[..],
            &fixture("good.minisig"),
            version("0.2.0"),
            version("0.1.0"),
            &[],
        );
        assert!(matches!(result, Err(SignatureError::UnknownKey)));
    }

    #[test]
    fn a_signature_past_four_kibibytes_is_refused_unread() {
        let mut big = fixture("good.minisig");
        big.resize(MAX_SIGNATURE_BYTES as usize + 1, b'\n');
        assert!(matches!(
            decode(&big),
            Err(SignatureError::SignatureTooLarge)
        ));
        let mut exact = fixture("good.minisig");
        exact.resize(MAX_SIGNATURE_BYTES as usize, b'\n');
        assert!(decode(&exact).is_ok());
    }

    #[test]
    fn a_signature_that_is_not_minisign_is_refused() {
        for text in [
            &b""[..],
            b"\xff\xfe",
            b"untrusted comment: x\n",
            b"hello\nworld\n",
        ] {
            assert!(
                matches!(decode(text), Err(SignatureError::Malformed)),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_comment_has_exactly_three_fields() {
        assert_eq!(
            parse_comment("zenkai/0.2.0/windows-x64"),
            Some(SignedRelease {
                version: version("0.2.0"),
                platform: "windows-x64"
            })
        );
        for comment in [
            "",
            "zenkai",
            "zenkai/0.2.0",
            "zenkai/0.2.0/",
            "zenkai/0.2.0/windows-x64/",
            "zenkai/0.2.0/windows-x64/extra",
            "Zenkai/0.2.0/windows-x64",
            "other/0.2.0/windows-x64",
            "zenkai/v0.2.0/windows-x64",
            "zenkai/0.2/windows-x64",
            "zenkai/00.2.0/windows-x64",
            " zenkai/0.2.0/windows-x64",
            "zenkai/0.2.0 /windows-x64",
            "zenkai//0.2.0/windows-x64",
            "timestamp:1763700000\tfile:zenkai-v0.2.0-windows-x64.zip",
        ] {
            assert_eq!(parse_comment(comment), None, "{comment:?}");
        }
    }
}
