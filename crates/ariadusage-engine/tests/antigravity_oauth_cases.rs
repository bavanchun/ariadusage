// Ported from CodexBar Tests/CodexBarTests/AntigravityOAuthCredentialsStoreTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_engine::providers::antigravity::*;
use std::path::{Path, PathBuf};
fn client_id(name: &str) -> String {
    format!("123456789012-{name}.apps.googleusercontent.com")
}
fn secret(character: char) -> String {
    format!("{}{}{}", "GOC", "SPX-", character.to_string().repeat(28))
}
fn make_client(name: &str, character: char) -> AntigravityOAuthClient {
    AntigravityOAuthClient::new(client_id(name), secret(character))
}
fn text_artifact(client: &AntigravityOAuthClient) -> String {
    format!(
        "out-build/vs/platform/cloudCode/common/oauthClient.js\nclientId=\"{}\";\nclientSecret=\"{}\";",
        client.client_id, client.client_secret
    )
}
fn native_artifact(client: &AntigravityOAuthClient) -> Vec<u8> {
    let mut data = vec![0xff];
    data.extend_from_slice(
        format!(
            "\0{}\0oauth_data\0{}\0",
            client.client_secret, client.client_id
        )
        .as_bytes(),
    );
    data
}
fn write_app(root: &Path, name: &str, bundle_id: &str, relative: &str, data: &[u8]) -> PathBuf {
    let bundle = root.join(name);
    let path = bundle.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(bundle.join("Contents/Info.plist"),
format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{bundle_id}</string></dict></plist>")).unwrap();
    std::fs::write(path, data).unwrap();
    bundle
}
fn make_go_instruction_binary_fixture(
    clients: &[&AntigravityOAuthClient],
    intel: bool,
    reverse_secrets: bool,
) -> Vec<u8> {
    let mut data = vec![0u8; 16384];
    let word = |buf: &mut [u8], offset: usize, value: u32| {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };
    let bytes = |buf: &mut [u8], offset: usize, values: &[u8]| {
        buf[offset..offset + values.len()].copy_from_slice(values);
    };
    let len_u32 = data.len() as u32;
    word(&mut data, 0, 0xFEED_FACF);
    word(&mut data, 4, if intel { 0x0100_0007 } else { 0x0100_000C });
    word(&mut data, 16, 1);
    word(&mut data, 20, 152);
    word(&mut data, 32, 0x19);
    word(&mut data, 36, 152);
    bytes(&mut data, 40, b"__TEXT");
    word(&mut data, 60, 1);
    // vmaddr = 0x100000000
    word(&mut data, 64, len_u32);
    word(&mut data, 80, len_u32);
    word(&mut data, 96, 1);
    bytes(&mut data, 104, b"__text");
    word(&mut data, 136, 512);
    word(&mut data, 140, 1);
    word(&mut data, 144, (clients.len() * 64) as u32);
    word(&mut data, 152, 512);
    for (index, client) in clients.iter().enumerate() {
        let instruction = 512 + index * 64;
        let secret_idx = if reverse_secrets {
            clients.len() - index - 1
        } else {
            index
        };
        let secret = 8192 + secret_idx * 128;
        let id = 12288 + index * 128;
        bytes(&mut data, secret, client.client_secret.as_bytes());
        bytes(&mut data, id, client.client_id.as_bytes());
        if intel {
            bytes(&mut data, instruction, &[0x48, 0xC7, 0x40, 0x08]);
            word(&mut data, instruction + 4, client.client_id.len() as u32);
            bytes(&mut data, instruction + 8, &[0x48, 0x8D, 0x0D]);
            word(&mut data, instruction + 11, (id - instruction - 15) as u32);
            bytes(
                &mut data,
                instruction + 15,
                &[0x48, 0x89, 0x08, 0x48, 0xC7, 0x40, 0x18],
            );
            word(&mut data, instruction + 22, 35);
            bytes(&mut data, instruction + 26, &[0x48, 0x8D, 0x0D]);
            word(
                &mut data,
                instruction + 29,
                (secret - instruction - 33) as u32,
            );
            bytes(&mut data, instruction + 33, &[0x48, 0x89, 0x48, 0x10]);
        } else {
            for (field, address, length) in [
                (0usize, id, client.client_id.len()),
                (1usize, secret, 35usize),
            ] {
                let offset = instruction + field * 16;
                let pages = ((address >> 12) - (offset >> 12)) as u32;
                word(
                    &mut data,
                    offset,
                    0x9000_0001 | ((pages & 3) << 29) | ((pages >> 2) << 5),
                );
                word(
                    &mut data,
                    offset + 4,
                    0x9100_0021 | (((address & 4095) as u32) << 10),
                );
                word(&mut data, offset + 8, 0xD280_0002 | ((length as u32) << 5));
                word(
                    &mut data,
                    offset + 12,
                    if field == 0 { 0xA900_0801 } else { 0xA901_0801 },
                );
            }
        }
    }
    data
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:6 oauth discovery declines unrelated binary identities without a matching record
#[test]
fn oauth_discovery_declines_unrelated_binary_identities_without_a_matching_record() {
    let mut data = vec![0xff];
    data.extend_from_slice(format!("{}\0{}", client_id("one"), client_id("two")).as_bytes());
    data.extend_from_slice(secret('a').as_bytes());
    data.extend_from_slice(secret('b').as_bytes());
    assert!(AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&data).is_none());
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:16 oauth discovery pairs the fields initialized together in Antigravity 2_19_1
#[test]
fn oauth_discovery_pairs_the_fields_initialized_together_in_antigravity_2_19_1() {
    for intel in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let client = make_client("desktop", 'a');
        let alternate = make_client("alternate", 'b');
        let _bundle = write_app(
            root.path(),
            "Antigravity.app",
            "com.google.antigravity",
            "Contents/Resources/bin/language_server",
            &make_go_instruction_binary_fixture(&[&client, &alternate], intel, false),
        );
        assert_eq!(
            AntigravityOAuthConfig::discover_client_from_installed_app(&[root.path()]),
            Some(client)
        );
    }
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:36 oauth discovery prefers the consumer client that agy signs in with
#[test]
fn oauth_discovery_prefers_the_consumer_client_that_agy_signs_in_with() {
    for intel in [false, true] {
        for consumer_first in [false, true] {
            let other = make_client("hub", 'a');
            let consumer = AntigravityOAuthClient::new(
                AntigravityOAuthConfig::CONSUMER_CLIENT_ID,
                secret('b'),
            );
            let clients = if consumer_first {
                vec![&consumer, &other]
            } else {
                vec![&other, &consumer]
            };
            let data = make_go_instruction_binary_fixture(&clients, intel, true);
            assert_eq!(
                AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&data),
                Some(consumer)
            );
        }
    }
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:50 credential expiry accepts integer fractional and absent JSON numbers
#[test]
fn credential_expiry_accepts_integer_fractional_and_absent_json_numbers() {
    for key in ["expiry_date", "expiresAt"] {
        for value in ["1700000000000", "1700000000000.5", "null"] {
            let data = format!("{{\"{key}\":{value}}}");
            let credentials =
                AntigravityOAuthCredentialsStore::decode_credentials(data.as_bytes()).unwrap();
            let actual = credentials.expiry_date.map(|date| {
                date.as_second() as f64 * 1000. + date.subsec_nanosecond() as f64 / 1_000_000.
            });
            assert_eq!(actual, value.parse::<f64>().ok());
        }
    }
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:57 missing or malformed id token claims retain the stored email
#[test]
fn missing_or_malformed_id_token_claims_retain_the_stored_email() {
    for token in [
        None,
        Some("invalid"),
        Some("header.e30.signature"),
        Some("header.%%%!.signature"),
    ] {
        let credentials = AntigravityOAuthCredentials::new(
            "",
            None,
            None,
            token.map(str::to_owned),
            Some(" User@example.com \n".into()),
            None,
            None,
            None,
        );
        assert_eq!(
            credentials.resolved_account_email().as_deref(),
            Some("User@example.com")
        );
    }
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:64 oauth discovery keeps a lone UTF8 pair beyond the text window
#[test]
fn oauth_discovery_keeps_a_lone_utf8_pair_beyond_the_text_window() {
    let client = make_client("legacy", 'a');
    let data = format!(
        "{}{}\0{}",
        " ".repeat(5000),
        client.client_id,
        client.client_secret
    );
    assert_eq!(
        AntigravityOAuthConfig::parse_client_from_installed_artifact_data(data.as_bytes()),
        Some(client)
    );
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:73 oauth discovery declines ambiguous text records
#[test]
fn oauth_discovery_declines_ambiguous_text_records() {
    let content = format!(
        "vs/platform/cloudCode/common/oauthClient.js\nclientId=\"{}\"; clientSecret=\"{}\";\nclientId=\"{}\"; clientSecret=\"{}\";",
        client_id("one"),
        secret('a'),
        client_id("two"),
        secret('b')
    );
    assert!(
        AntigravityOAuthConfig::parse_client_from_installed_artifact_data(content.as_bytes())
            .is_none()
    );
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:83 oauth discovery rejects malformed MachO metadata
#[test]
fn oauth_discovery_rejects_malformed_macho_metadata() {
    for intel in [false, true] {
        for offset in [20, 36, 96, 152] {
            let one = make_client("one", 'a');
            let two = make_client("two", 'a');
            let mut data = make_go_instruction_binary_fixture(&[&one, &two], intel, false);
            data[offset..offset + 4].copy_from_slice(&[0xff; 4]);
            assert!(
                AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&data).is_none()
            );
        }
    }
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:95 oauth discovery does not cross pair broken records
#[test]
fn oauth_discovery_does_not_cross_pair_broken_records() {
    for intel in [false, true] {
        let one = make_client("one", 'a');
        let two = make_client("two", 'a');
        let mut data = make_go_instruction_binary_fixture(&[&one, &two], intel, false);
        data[if intel { 548 } else { 543 }] = 0;
        data[if intel { 593 } else { 591 }] = 0;
        assert!(AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&data).is_none());
    }
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:109 oauth discovery accepts a binary Data slice
#[test]
fn oauth_discovery_accepts_a_binary_data_slice() {
    let client = make_client("desktop", 'a');
    let alternate = make_client("alternate", 'b');
    let mut data = vec![0];
    data.extend(make_go_instruction_binary_fixture(
        &[&client, &alternate],
        false,
        false,
    ));
    assert_eq!(
        AntigravityOAuthConfig::parse_client_from_installed_artifact_data(&data[1..]),
        Some(client)
    );
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:181 oauth client discovery reads renamed legacy bundle
#[test]
fn oauth_client_discovery_reads_renamed_legacy_bundle() {
    let client = make_client("legacy", 'a');
    let artifact = text_artifact(&client).into_bytes();
    let root = tempfile::tempdir().unwrap();
    let _bundle = write_app(
        root.path(),
        "Antigravity 2.app",
        "com.google.antigravity-ide",
        "Contents/Resources/app/out/main.js",
        &artifact,
    );
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[root.path()]),
        Some(client)
    );
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:206 oauth client discovery reads renamed gemini bundle identifier
#[test]
fn oauth_client_discovery_reads_renamed_gemini_bundle_identifier() {
    let client = make_client("gemini", 'f');
    let artifact = text_artifact(&client).into_bytes();
    let root = tempfile::tempdir().unwrap();
    let _bundle = write_app(
        root.path(),
        "Google Gemini.app",
        "com.google.GeminiMacOS",
        "Contents/Resources/app/out/main.js",
        &artifact,
    );
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[root.path()]),
        Some(client)
    );
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:231 oauth client discovery reads gemini native binary artifact
#[test]
fn oauth_client_discovery_reads_gemini_native_binary_artifact() {
    let client = make_client("gemini-native", 'g');
    let artifact = native_artifact(&client);
    let root = tempfile::tempdir().unwrap();
    let _bundle = write_app(
        root.path(),
        "Gemini.app",
        "com.google.GeminiMacOS",
        "Contents/MacOS/Gemini",
        &artifact,
    );
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[root.path()]),
        Some(client)
    );
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:257 oauth client discovery skips gemini app with foreign bundle identifier
#[test]
fn oauth_client_discovery_skips_gemini_app_with_foreign_bundle_identifier() {
    let client = make_client("foreign", 'h');
    let artifact = native_artifact(&client);
    let root = tempfile::tempdir().unwrap();
    let _bundle = write_app(
        root.path(),
        "Gemini.app",
        "com.example.other-gemini",
        "Contents/MacOS/Gemini",
        &artifact,
    );
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[root.path()]),
        None
    );
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:283 oauth client discovery reads standalone antigravity 2 bundle
#[test]
fn oauth_client_discovery_reads_standalone_antigravity_2_bundle() {
    let client = make_client("standalone", 'b');
    let alternate = make_client("alternate", 'c');
    let artifact = make_go_instruction_binary_fixture(&[&client, &alternate], false, true);
    let root = tempfile::tempdir().unwrap();
    let _bundle = write_app(
        root.path(),
        "Antigravity.app",
        "com.google.antigravity",
        "Contents/Resources/bin/language_server",
        &artifact,
    );
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[root.path()]),
        Some(client)
    );
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:308 oauth client discovery reads antigravity extension language server
#[test]
fn oauth_client_discovery_reads_antigravity_extension_language_server() {
    let client = make_client("extension", 'd');
    let stale = make_client("stale", 'e');
    let root = tempfile::tempdir().unwrap();
    write_app(
        root.path(),
        "Antigravity IDE.app",
        "com.google.antigravity-ide",
        "Contents/Resources/app/out/main.js",
        text_artifact(&stale).as_bytes(),
    );
    let _bundle = write_app(
        root.path(),
        "Antigravity IDE.app",
        "com.google.antigravity-ide",
        "Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm",
        &native_artifact(&client),
    );
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[root.path()]),
        Some(client)
    );
}

// upstream: AntigravityOAuthCredentialsStoreTests.swift:347 oauth client discovery pairs lone binary secret with trailing client id
#[test]
fn oauth_client_discovery_pairs_lone_binary_secret_with_trailing_client_id() {
    let client = make_client("standalone", 'b');
    let alternate = client_id("alternate");
    let mut artifact = make_go_instruction_binary_fixture(&[&client], false, false);
    artifact[12000..12000 + alternate.len()].copy_from_slice(alternate.as_bytes());
    let root = tempfile::tempdir().unwrap();
    let _bundle = write_app(
        root.path(),
        "Antigravity.app",
        "com.google.antigravity",
        "Contents/Resources/bin/language_server",
        &artifact,
    );
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[root.path()]),
        Some(client)
    );
}
