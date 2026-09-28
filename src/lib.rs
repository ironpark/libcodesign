//! C ABI for the apple-codesign library. No CLI parsing, global logger or subprocesses.
use apple_codesign::{
    cli::certificate_source::{CertificateSource, P12SigningKey, PemSigningKey},
    notarization::Notarizer,
    stapling::Stapler,
    CodeSignatureFlags, SettingsScope, SigningSettings, UnifiedSigner,
};
use serde::Deserialize;
use std::{
    ffi::{c_char, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Options {
    p12_file: String,
    p12_password: String,
    p12_password_file: String,
    pem_file: String,
    api_key_file: String,
    entitlements_file: String,
    notarize_timeout_secs: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: String,
    path: String,
    options: Options,
}

fn execute(request: Request, timestamp: bool) -> Result<(), Box<dyn std::error::Error>> {
    if request.path.is_empty() {
        return Err("a target path is required".into());
    }
    let path = Path::new(&request.path);
    let opts = request.options;
    match request.operation.as_str() {
        "sign" => {
            if opts.p12_file.is_empty() && opts.pem_file.is_empty() {
                return Err("a P12 or PEM signing certificate is required".into());
            }
            let mut source = CertificateSource::default();
            if !opts.p12_file.is_empty() {
                // Resolve the password here: never prompt on stdin from an FFI call.
                let password = if opts.p12_password_file.is_empty() {
                    opts.p12_password
                } else {
                    std::fs::read_to_string(&opts.p12_password_file)?
                        .lines()
                        .next()
                        .ok_or("password file is empty")?
                        .to_owned()
                };
                source.p12_key = Some(P12SigningKey {
                    path: Some(opts.p12_file.into()),
                    password: Some(password),
                    password_path: None,
                });
            }
            if !opts.pem_file.is_empty() {
                source.pem_path_key = Some(PemSigningKey {
                    paths: vec![opts.pem_file.into()],
                });
            }
            let certs = source.resolve_certificates(false)?;
            // Reject certificate-only PEM inputs instead of silently producing ad-hoc signatures.
            let key = certs.private_key()?;
            let mut settings = SigningSettings::default();
            certs.load_into_signing_settings(&mut settings)?;
            settings.set_team_id_from_signing_certificate();
            settings.set_code_signature_flags(SettingsScope::Main, CodeSignatureFlags::RUNTIME);
            if path.is_dir() {
                // A bundle's main scope flags reach its main executable only;
                // nested code keeps the flags it was last signed with. The
                // notary wants the hardened runtime on every executable, so
                // each Mach-O is given it first, untimestamped, and signing
                // the bundle then preserves it along with any entitlements.
                for file in macho_files(path)? {
                    UnifiedSigner::new(settings.clone()).sign_path_in_place(&file)?;
                }
            }
            if timestamp {
                settings.set_time_stamp_url("http://timestamp.apple.com/ts01")?;
            }
            // Main scope reaches only the main executable; nested code keeps
            // the entitlements it already carries, as it does without a file.
            if !opts.entitlements_file.is_empty() {
                settings.set_entitlements_xml(
                    SettingsScope::Main,
                    std::fs::read_to_string(&opts.entitlements_file)?,
                )?;
            }
            UnifiedSigner::new(settings).sign_path_in_place(path)?;
            key.finish()?;
        }
        "submit" => {
            if opts.api_key_file.is_empty() {
                return Err("an App Store Connect API key file is required".into());
            }
            // Zero keeps the ten minutes callers before the option had.
            let timeout = match opts.notarize_timeout_secs {
                0 => 600,
                secs => secs,
            };
            Notarizer::from_api_key(Path::new(&opts.api_key_file))?
                .notarize_path(path, Some(Duration::from_secs(timeout)))?;
        }
        "staple" => Stapler::new()?.staple_path(path)?,
        _ => return Err("unsupported signing operation".into()),
    }
    Ok(())
}

/// Lists the Mach-O files under dir, not following symlinks.
fn macho_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            found.extend(macho_files(&entry.path())?);
        } else if kind.is_file() && is_macho(&entry.path())? {
            found.push(entry.path());
        }
    }
    Ok(found)
}

fn is_macho(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut header = [0u8; 8];
    match std::fs::File::open(path)?.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(e) => return Err(e),
    }
    let magic = u32::from_be_bytes(header[..4].try_into().unwrap());
    Ok(match magic {
        0xfeedface | 0xfeedfacf | 0xcefaedfe | 0xcffaedfe => true,
        // Java class files share the universal magic; there the next word is
        // a class file version, 45 or more, where a universal binary counts
        // its few architectures.
        0xcafebabe | 0xcafebabf => u32::from_be_bytes(header[4..].try_into().unwrap()) < 45,
        _ => false,
    })
}

/// Returns NULL on success; otherwise an owned error string, released with zapp_rcodesign_free.
/// The request must be a valid NUL-terminated UTF-8 JSON string for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn zapp_rcodesign_run(request: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(
        || -> Result<(), Box<dyn std::error::Error>> {
            if request.is_null() {
                return Err("null signing request".into());
            }
            execute(
                serde_json::from_str(CStr::from_ptr(request).to_str()?)?,
                true,
            )
        },
    ));
    let error = match result {
        Ok(Ok(())) => return std::ptr::null_mut(),
        Ok(Err(error)) => error.to_string(),
        Err(_) => "Rust signing library panicked".to_owned(),
    };
    CString::new(error.replace('\0', "\\0")).unwrap().into_raw()
}

/// Free a non-NULL error returned by zapp_rcodesign_run exactly once.
#[no_mangle]
pub unsafe extern "C" fn zapp_rcodesign_free(error: *mut c_char) {
    if !error.is_null() {
        drop(CString::from_raw(error));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn error(json: &str) -> String {
        let input = CString::new(json).unwrap();
        unsafe {
            let result = zapp_rcodesign_run(input.as_ptr());
            assert!(!result.is_null());
            let text = CStr::from_ptr(result).to_string_lossy().into_owned();
            zapp_rcodesign_free(result);
            text
        }
    }
    #[test]
    fn ffi_rejects_invalid_requests_without_unwinding() {
        assert!(!error("not json").is_empty());
        assert!(error(r#"{"operation":"unknown","path":"x","options":{}}"#).contains("unsupported"));
        assert!(error(r#"{"operation":"sign","path":"x","options":{}}"#).contains("certificate"));
        assert!(error(
            r#"{"operation":"submit","path":"x","options":{"notarize_timeout_secs":3600}}"#
        )
        .contains("API key"));
        unsafe {
            let result = zapp_rcodesign_run(std::ptr::null());
            assert!(!result.is_null());
            zapp_rcodesign_free(result);
            zapp_rcodesign_free(std::ptr::null_mut());
        }
    }
    #[test]
    fn signs_macho_with_certificate_and_hardened_runtime() {
        use apple_codesign::{
            create_self_signed_code_signing_certificate, macho_builder::MachOBuilder,
            verify_macho_data, CertificateProfile, MachFile,
        };
        use x509_certificate::{EcdsaCurve, KeyAlgorithm};
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("certificate.pem");
        let (cert, key) = create_self_signed_code_signing_certificate(
            KeyAlgorithm::Ecdsa(EcdsaCurve::Secp256r1),
            CertificateProfile::DeveloperIdApplication,
            "TESTTEAM",
            "Zapp Test",
            "US",
            chrono::Duration::hours(1),
        )
        .unwrap();
        let private = pem::encode(&pem::Pem::new(
            "PRIVATE KEY",
            key.to_pkcs8_one_asymmetric_key_der().to_vec(),
        ));
        std::fs::write(&cert_path, format!("{}{}", cert.encode_pem(), private)).unwrap();
        let entitlements_path = dir.path().join("app.entitlements");
        std::fs::write(
            &entitlements_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>com.apple.security.cs.allow-jit</key><true/></dict></plist>"#,
        )
        .unwrap();
        for (name, builder) in [
            ("amd64", MachOBuilder::new_x86_64(2)),
            ("arm64", MachOBuilder::new_aarch64(2)),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, builder.write_macho().unwrap()).unwrap();
            execute(
                Request {
                    operation: "sign".into(),
                    path: path.to_str().unwrap().into(),
                    options: Options {
                        pem_file: cert_path.to_str().unwrap().into(),
                        entitlements_file: entitlements_path.to_str().unwrap().into(),
                        ..Default::default()
                    },
                },
                false,
            )
            .unwrap();
            let data = std::fs::read(&path).unwrap();
            assert!(verify_macho_data(&data).is_empty());
            let macho = MachFile::parse(&data).unwrap();
            for binary in macho.iter_macho() {
                let signature = binary.code_signature().unwrap().unwrap();
                assert!(signature
                    .code_directory()
                    .unwrap()
                    .unwrap()
                    .flags
                    .contains(CodeSignatureFlags::RUNTIME));
                assert!(signature
                    .entitlements()
                    .unwrap()
                    .unwrap()
                    .as_str()
                    .contains("com.apple.security.cs.allow-jit"));
            }
        }
    }

    #[test]
    fn signs_nested_code_with_hardened_runtime() {
        use apple_codesign::{
            create_self_signed_code_signing_certificate, macho_builder::MachOBuilder,
            CertificateProfile, MachFile,
        };
        use x509_certificate::{EcdsaCurve, KeyAlgorithm};
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("certificate.pem");
        let (cert, key) = create_self_signed_code_signing_certificate(
            KeyAlgorithm::Ecdsa(EcdsaCurve::Secp256r1),
            CertificateProfile::DeveloperIdApplication,
            "TESTTEAM",
            "Zapp Test",
            "US",
            chrono::Duration::hours(1),
        )
        .unwrap();
        let private = pem::encode(&pem::Pem::new(
            "PRIVATE KEY",
            key.to_pkcs8_one_asymmetric_key_der().to_vec(),
        ));
        std::fs::write(&cert_path, format!("{}{}", cert.encode_pem(), private)).unwrap();
        let entitlements_path = dir.path().join("app.entitlements");
        std::fs::write(
            &entitlements_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>com.apple.security.cs.allow-jit</key><true/></dict></plist>"#,
        )
        .unwrap();

        let app = dir.path().join("Demo.app");
        let plist = |exe: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>{exe}</string><key>CFBundleIdentifier</key><string>com.example.{exe}</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>"#
            )
        };
        let binaries = [
            "Contents/MacOS/Demo",
            "Contents/MacOS/tool",
            "Contents/Frameworks/libx.dylib",
            "Contents/Frameworks/Helper.app/Contents/MacOS/Helper",
            "Contents/Frameworks/Helper.app/Contents/Frameworks/liby.dylib",
        ];
        for rel in binaries {
            let path = app.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, MachOBuilder::new_aarch64(2).write_macho().unwrap()).unwrap();
        }
        std::fs::write(app.join("Contents/Info.plist"), plist("Demo")).unwrap();
        std::fs::write(
            app.join("Contents/Frameworks/Helper.app/Contents/Info.plist"),
            plist("Helper"),
        )
        .unwrap();

        execute(
            Request {
                operation: "sign".into(),
                path: app.to_str().unwrap().into(),
                options: Options {
                    pem_file: cert_path.to_str().unwrap().into(),
                    entitlements_file: entitlements_path.to_str().unwrap().into(),
                    ..Default::default()
                },
            },
            false,
        )
        .unwrap();
        for rel in binaries {
            let data = std::fs::read(app.join(rel)).unwrap();
            for binary in MachFile::parse(&data).unwrap().iter_macho() {
                let signature = binary.code_signature().unwrap().unwrap();
                let flags = signature.code_directory().unwrap().unwrap().flags;
                assert!(
                    flags.contains(CodeSignatureFlags::RUNTIME),
                    "{rel}: {flags:?}"
                );
                // The entitlements file is the main executable's alone.
                let entitled = signature.entitlements().unwrap().is_some();
                assert_eq!(entitled, rel == "Contents/MacOS/Demo", "{rel}");
            }
        }
    }

    #[test]
    fn missing_p12_is_an_error_without_prompting() {
        assert!(!error(
            r#"{"operation":"sign","path":"x","options":{"p12_file":"/nonexistent/zapp.p12"}}"#
        )
        .is_empty());
    }
}
