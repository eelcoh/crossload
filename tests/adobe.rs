use base64::{engine::general_purpose::STANDARD, Engine};
use crossload::{adobe::Store, epub};
use openssl::{
    asn1::Asn1Time,
    hash::MessageDigest,
    pkcs12::Pkcs12,
    pkey::PKey,
    rsa::{Padding, Rsa},
    symm::{encrypt, Cipher},
    x509::{X509NameBuilder, X509},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Write},
    os::unix::fs::{symlink, PermissionsExt},
    path::Path,
};
use tempfile::TempDir;
use zip::{write::SimpleFileOptions, ZipWriter};

const USER: &str = "urn:uuid:11111111-2222-3333-4444-555555555555";
const ACSM: &[u8] = b"<fulfillmentToken xmlns='http://ns.adobe.com/adept'><resourceItemInfo><metadata><format>application/epub+zip</format></metadata></resourceItemInfo></fulfillmentToken>";
const CHAPTER: &[u8] = b"<html xmlns='http://www.w3.org/1999/xhtml'><head><title>Test</title></head><body><p>Synthetic ADEPT book</p></body></html>";
struct Fixture {
    dir: TempDir,
    rsa: Rsa<openssl::pkey::Private>,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let rsa = Rsa::generate(1024).unwrap();
        let key = PKey::from_rsa(rsa.clone()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "xteink synthetic test")
            .unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        let cert = cert.build();
        let mut p12 = Pkcs12::builder();
        p12.name("test").pkey(&key).cert(&cert);
        let p12 = p12
            .build2(&STANDARD.encode([0x42; 16]))
            .unwrap()
            .to_der()
            .unwrap();
        fs::write(dir.path().join("devicesalt"), [0x42; 16]).unwrap();
        fs::write(dir.path().join("device.xml"), "<adept:deviceInfo xmlns:adept='http://ns.adobe.com/adept'><adept:deviceClass>Desktop</adept:deviceClass><adept:deviceSerial>test</adept:deviceSerial><adept:deviceName>Test</adept:deviceName><adept:deviceType>standalone</adept:deviceType><adept:fingerprint>test-fingerprint</adept:fingerprint><adept:version name='hobbes' value='10.0.4'/><adept:version name='clientOS' value='Linux'/><adept:version name='clientLocale' value='en'/></adept:deviceInfo>").unwrap();
        fs::write(dir.path().join("activation.xml"), format!("<activation_info xmlns:adept='http://ns.adobe.com/adept'><adept:credentials><adept:user>{USER}</adept:user><adept:username method='anonymous'/><adept:pkcs12>{}</adept:pkcs12><adept:authenticationCertificate>{}</adept:authenticationCertificate><adept:privateLicenseKey>{}</adept:privateLicenseKey></adept:credentials><activationToken><device>urn:uuid:test-device</device><fingerprint>test-fingerprint</fingerprint></activationToken></activation_info>", STANDARD.encode(p12), STANDARD.encode(cert.to_der().unwrap()), STANDARD.encode(key.private_key_to_pkcs8().unwrap()))).unwrap();
        Self { dir, rsa }
    }
    fn install(&self, root: &Path) -> Store {
        let store = Store::open(root).unwrap();
        store.import_activation(self.dir.path()).unwrap();
        store
    }
    fn encrypted_book(&self, broken: bool) -> Vec<u8> {
        let mut compressor =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        compressor.write_all(CHAPTER).unwrap();
        let compressed = compressor.finish().unwrap();
        let mut chapter = vec![0x22; 16];
        chapter.extend(
            encrypt(
                Cipher::aes_128_cbc(),
                &[0x33; 16],
                Some(&[0x22; 16]),
                &compressed,
            )
            .unwrap(),
        );
        if broken {
            chapter.truncate(17);
        }
        let mut wrapped = vec![0; 128];
        self.rsa
            .public_encrypt(&[0x33; 16], &mut wrapped, Padding::PKCS1)
            .unwrap();
        let rights = format!("<adept:rights xmlns:adept='http://ns.adobe.com/adept'><licenseToken><user>{USER}</user><encryptedKey>{}</encryptedKey></licenseToken></adept:rights>", STANDARD.encode(wrapped));
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in [
            ("mimetype", b"application/epub+zip".as_slice()),
            ("META-INF/container.xml", b"<container><rootfiles><rootfile full-path='OPS/package.opf'/></rootfiles></container>"),
            ("OPS/package.opf", b"<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>Synthetic / book</dc:title></metadata><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>"),
            ("META-INF/rights.xml", rights.as_bytes()),
            ("META-INF/encryption.xml", b"<encryption><EncryptedData><EncryptionMethod Algorithm='http://www.w3.org/2001/04/xmlenc#aes128-cbc'/><CipherData><CipherReference URI='OPS/chapter.xhtml'/></CipherData></EncryptedData></encryption>"),
            ("OPS/chapter.xhtml", chapter.as_slice()),
        ] {
            zip.start_file(name, SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap(); zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    fn cache(&self, root: &Path, broken: bool) -> std::path::PathBuf {
        let cache = root
            .join("acsm")
            .join(format!("{:x}", Sha256::digest(ACSM)));
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("receipt.xml"), "<book/>").unwrap();
        fs::write(cache.join("original.epub"), self.encrypted_book(broken)).unwrap();
        cache
    }
}
#[test]
fn activation_import_is_private_validated_and_never_overwritten() {
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    let store = fixture.install(&root.path().join("state"));
    assert!(store.status().unwrap());
    assert_eq!(
        fs::metadata(root.path().join("state/activation/devicesalt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(store.import_activation(fixture.dir.path()).is_err());
    assert!(Store::open(&root.path().join("state")).is_err());
}
#[test]
fn mismatched_salt_does_not_install() {
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    fs::write(fixture.dir.path().join("devicesalt"), [0; 16]).unwrap();
    let store = Store::open(root.path()).unwrap();
    assert!(store.import_activation(fixture.dir.path()).is_err());
    assert!(!store.status().unwrap());
}
#[test]
fn zip_export_import_and_duplicate_rejection() {
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("activation.zip");
    let mut zip = ZipWriter::new(fs::File::create(&path).unwrap());
    for name in ["device.xml", "activation.xml", "devicesalt"] {
        zip.start_file(format!("export/{name}"), SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&fs::read(fixture.dir.path().join(name)).unwrap())
            .unwrap();
    }
    zip.finish().unwrap();
    let store = Store::open(&root.path().join("state")).unwrap();
    store.import_activation(&path).unwrap();
    assert!(store.status().unwrap());
    let mut zip = ZipWriter::new(fs::File::create(&path).unwrap());
    for name in ["device.xml", "other/device.xml"] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(b"<x/>").unwrap();
    }
    zip.finish().unwrap();
    let store = Store::open(&root.path().join("other-state")).unwrap();
    assert!(store.import_activation(&path).is_err());
}
#[test]
fn cached_adept_decrypts_and_never_overwrites() {
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let store = fixture.install(&state);
    let cache = fixture.cache(&state, false);
    let original = fs::read(cache.join("original.epub")).unwrap();
    let input = root.path().join("book.acsm");
    fs::write(&input, ACSM).unwrap();
    let output = root.path().join("books");
    let book = store.import(&input, &output).unwrap();
    epub::validate(&fs::read(&book).unwrap()).unwrap();
    assert!(book
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("Synthetic _ book"));
    assert_eq!(original, fs::read(cache.join("original.epub")).unwrap());
    assert!(store.import(&input, &output).is_err());
    assert_eq!(fs::read_dir(output).unwrap().count(), 1);
}
#[test]
fn corrupt_encryption_never_publishes() {
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let store = fixture.install(&state);
    fixture.cache(&state, true);
    let input = root.path().join("book.acsm");
    fs::write(&input, ACSM).unwrap();
    let output = root.path().join("books");
    assert!(store.import(&input, &output).is_err());
    assert_eq!(fs::read_dir(output).unwrap().count(), 0);
}
#[test]
fn ambiguous_fulfillment_is_not_replayed() {
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let store = fixture.install(&state);
    let cache = state
        .join("acsm")
        .join(format!("{:x}", Sha256::digest(ACSM)));
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("fulfillment-started"), "started").unwrap();
    let input = root.path().join("book.acsm");
    fs::write(&input, ACSM).unwrap();
    let error = store
        .import(&input, &root.path().join("books"))
        .unwrap_err();
    assert!(error.to_string().contains("unknown"));
}
#[test]
fn activation_source_symlinks_rejected() {
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    fs::rename(
        fixture.dir.path().join("devicesalt"),
        fixture.dir.path().join("salt"),
    )
    .unwrap();
    symlink("salt", fixture.dir.path().join("devicesalt")).unwrap();
    let store = Store::open(root.path()).unwrap();
    assert!(store.import_activation(fixture.dir.path()).is_err());
}
#[test]
fn non_acsm_and_pdf_rejected_before_network() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("state")).unwrap();
    let input = root.path().join("book.acsm");
    for data in [
        "<html/>",
        "<fulfillmentToken><format>application/pdf</format></fulfillmentToken>",
    ] {
        fs::write(&input, data).unwrap();
        assert!(store.import(&input, &root.path().join("books")).is_err());
    }
    assert!(!root.path().join("state/acsm").exists());
}

#[test]
fn fulfillment_download_retry_reuses_receipt_and_decrypts() {
    use std::{
        io::Read,
        net::TcpListener,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let activation = fixture.dir.path().join("activation.xml");
    let xml = fs::read_to_string(&activation).unwrap().replace("</activation_info>", &format!("<adept:operatorURLList><adept:operatorURL>{url}/Fulfill</adept:operatorURL></adept:operatorURLList><adept:licenseServices><adept:licenseServiceInfo><adept:licenseURL>{url}/license</adept:licenseURL><adept:certificate>synthetic</adept:certificate></adept:licenseServiceInfo></adept:licenseServices></activation_info>"));
    fs::write(activation, xml).unwrap();
    let book = fixture.encrypted_book(false);
    let mut wrapped = vec![0; 128];
    fixture
        .rsa
        .public_encrypt(&[0x33; 16], &mut wrapped, Padding::PKCS1)
        .unwrap();
    let reply = format!("<envelope><fulfillmentResult><resourceItemInfo><src>{url}/book.epub</src><resource>urn:uuid:test-book</resource><metadata xmlns:dc='http://purl.org/dc/elements/1.1/'><dc:title>Synthetic book</dc:title><dc:format>application/epub+zip</dc:format></metadata><licenseToken><licenseURL>{url}/license</licenseURL><user>{USER}</user><encryptedKey>{}</encryptedKey></licenseToken></resourceItemInfo></fulfillmentResult></envelope>", STANDARD.encode(wrapped));
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        for step in 0..3 {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "Missing HTTP request {step}");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let header_end = loop {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break request.len();
                }
            };
            let header = String::from_utf8_lossy(&request).to_string();
            let length = header
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            request.resize(header_end + length, 0);
            stream.read_exact(&mut request[header_end..]).unwrap();
            if step == 0 {
                assert!(header.starts_with("POST /Fulfill "));
                let body = std::str::from_utf8(&request[header_end..]).unwrap();
                assert!(body.contains("<hmac>test-hmac</hmac>"));
                assert!(body.contains("adept:signature"));
            } else {
                assert!(header.starts_with("GET /book.epub "));
            }
            let bytes: &[u8] = if step == 0 {
                reply.as_bytes()
            } else if step == 1 {
                b"try again"
            } else {
                &book
            };
            write!(stream, "HTTP/1.1 {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", if step == 1 { "503 Unavailable" } else { "200 OK" }, bytes.len()).unwrap();
            stream.write_all(bytes).unwrap();
        }
    });
    let root = tempfile::tempdir().unwrap();
    let store = fixture.install(&root.path().join("state"));
    let input = root.path().join("book.acsm");
    fs::write(&input, format!("<fulfillmentToken xmlns='http://ns.adobe.com/adept'><operatorURL>{url}</operatorURL><hmac>test-hmac</hmac></fulfillmentToken>")).unwrap();
    let error = store
        .import(&input, &root.path().join("books"))
        .unwrap_err();
    assert!(format!("{error:#}").contains("503"), "{error:#}");
    let path = store.import(&input, &root.path().join("books")).unwrap();
    epub::validate(&fs::read(path).unwrap()).unwrap();
    server.join().unwrap();
}
