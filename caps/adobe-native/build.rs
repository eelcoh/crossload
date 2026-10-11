use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=native");
    let zroot = PathBuf::from(env::var("DEP_Z_ROOT").expect("zlib build root"));
    let mut zip = cmake::Config::new("native/vendor/libzip");
    for option in [
        "BUILD_SHARED_LIBS",
        "BUILD_TOOLS",
        "BUILD_REGRESS",
        "BUILD_OSSFUZZ",
        "BUILD_EXAMPLES",
        "BUILD_DOC",
        "ENABLE_COMMONCRYPTO",
        "ENABLE_GNUTLS",
        "ENABLE_MBEDTLS",
        "ENABLE_OPENSSL",
        "ENABLE_WINDOWS_CRYPTO",
        "ENABLE_BZIP2",
        "ENABLE_LZMA",
        "ENABLE_ZSTD",
    ] {
        zip.define(option, "OFF");
    }
    let zip = zip
        .define("CMAKE_INSTALL_LIBDIR", "lib")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("ZLIB_INCLUDE_DIR", zroot.join("include"))
        .define("ZLIB_LIBRARY", zroot.join("lib/libz.a"))
        .build();
    let mut native = cc::Build::new();
    native
        .cpp(true)
        .std("c++11")
        .warnings(false)
        .include("native/vendor/libgourou/include")
        .include("native/vendor/libgourou/utils")
        .include("native/vendor/updfparser/include")
        .include("native/vendor/pugixml/src")
        .include(zip.join("include"))
        .include(zroot.join("include"))
        .include(env::var("DEP_OPENSSL_INCLUDE").expect("OpenSSL headers"))
        .include(env::var("DEP_CURL_INCLUDE").expect("curl headers"))
        .file("native/bridge.cpp")
        .file("native/vendor/libgourou/utils/drmprocessorclientimpl.cpp")
        .file("native/vendor/pugixml/src/pugixml.cpp")
        .file("native/vendor/updfparser/src/uPDFParser.cpp")
        .file("native/vendor/updfparser/src/uPDFTypes.cpp");
    for source in [
        "libgourou",
        "user",
        "device",
        "fulfillment_item",
        "loan_token",
        "bytearray",
    ] {
        native.file(format!("native/vendor/libgourou/src/{source}.cpp"));
    }
    native.compile("xteink_gourou");
    println!(
        "cargo:rustc-link-search=native={}",
        zip.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=zip");
}
