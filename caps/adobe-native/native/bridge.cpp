// SPDX-License-Identifier: MIT
// All native exceptions terminate at this ABI; Rust serializes entry points.
#include <openssl/opensslv.h>
#include <openssl/pem.h>
#include <openssl/pkcs12.h>
#include <curl/curl.h>
#include <libgourou.h>
#include <libgourou_common.h>
#include <user.h>
#include <device.h>
#include <drmprocessorclientimpl.h>
#include <memory>
#include <fstream>
#include <cstdio>
#include <cstring>
#include <stdexcept>

static std::string ca_file;
static std::string ca_dir;
extern "C" const char* xteink_ca_file() { return ca_file.empty() ? NULL : ca_file.c_str(); }
extern "C" const char* xteink_ca_dir() { return ca_dir.empty() ? NULL : ca_dir.c_str(); }

static void save(pugi::xml_document& doc, const std::string& path) {
    const std::string temporary = path + ".tmp";
    if (!doc.save_file(temporary.c_str()) || rename(temporary.c_str(), path.c_str()))
        throw std::runtime_error("Could not save fulfillment receipt");
}

extern "C" int xteink_adobe(int operation, const char* activation, const char* input,
                            const char* output, const char* cert_file, const char* cert_dir,
                            char* error, size_t capacity) noexcept {
    try {
        ca_file = cert_file; ca_dir = cert_dir;
        gourou::DRMProcessor::setLogLevel(0);
        if (curl_global_init(CURL_GLOBAL_DEFAULT) != CURLE_OK)
            throw std::runtime_error("Could not initialize HTTPS");
        DRMProcessorClientImpl client;
        std::string root(activation);
        gourou::DRMProcessor processor(&client, root + "/device.xml", root + "/activation.xml", root + "/devicesalt");
        if (operation == 0) {
            // Parsing alone cannot detect a mismatched device salt. Validate the
            // encrypted PKCS12 using the same password as fulfillment signing.
            auto bytes = gourou::ByteArray::fromBase64(processor.getUser()->getPKCS12());
            const unsigned char* ptr = bytes.data();
            std::unique_ptr<PKCS12, decltype(&PKCS12_free)> p12(d2i_PKCS12(NULL, &ptr, bytes.length()), PKCS12_free);
            auto salt = processor.getDevice()->getDeviceKey();
            std::string password = gourou::ByteArray(salt, 16).toBase64();
            if (!p12 || PKCS12_verify_mac(p12.get(), password.c_str(), password.size()) != 1)
                throw std::runtime_error("Activation PKCS12 and devicesalt do not match");
            EVP_PKEY* auth_key = NULL;
            X509* certificate = NULL;
            const int parsed = PKCS12_parse(p12.get(), password.c_str(), &auth_key, &certificate, NULL);
            std::unique_ptr<EVP_PKEY, decltype(&EVP_PKEY_free)> auth(auth_key, EVP_PKEY_free);
            std::unique_ptr<X509, decltype(&X509_free)> cert(certificate, X509_free);
            if (!parsed || !auth || EVP_PKEY_get_size(auth.get()) != 128)
                throw std::runtime_error("Unsupported activation signing key; expected 1024-bit ADEPT RSA");
            auto license_bytes = gourou::ByteArray::fromBase64(processor.getUser()->getPrivateLicenseKey());
            ptr = license_bytes.data();
            std::unique_ptr<EVP_PKEY, decltype(&EVP_PKEY_free)> license(d2i_AutoPrivateKey(NULL, &ptr, license_bytes.length()), EVP_PKEY_free);
            if (!license || EVP_PKEY_get_size(license.get()) != 128)
                throw std::runtime_error("Unsupported activation license key; expected 1024-bit ADEPT RSA");
        } else if (operation == 1) {
            std::unique_ptr<gourou::FulfillmentItem> item(processor.fulfill(input));
            pugi::xml_document receipt;
            auto book = receipt.append_child("book");
            book.append_child("url").text().set(item->getDownloadURL().c_str());
            book.append_child("title").text().set(item->getMetadata("title").c_str());
            book.append_child("format").text().set(item->getMetadata("format").c_str());
            book.append_child("rights").text().set(item->getRights().c_str());
            if (auto loan = item->getLoanToken()) {
                auto node = book.append_child("loan");
                for (auto key : {"id", "operatorURL", "validity"})
                    node.append_child(key).text().set(loan->getProperty(key).c_str());
            }
            save(receipt, output);
        } else if (operation == 2) {
            pugi::xml_document receipt;
            if (!receipt.load_file(input)) throw std::runtime_error("Invalid fulfillment receipt");
            auto book = receipt.child("book");
            if (std::string(book.child_value("format")).find("application/pdf") != std::string::npos)
                throw std::runtime_error("PDF fulfillment is not supported yet; receipt retained");
            int fd = gourou::createNewFile(output, true);
            try { client.sendHTTPRequest(book.child_value("url"), "", "", NULL, fd, false); }
            catch (...) { close(fd); throw; }
            close(fd);
            void* archive = client.zipOpen(output);
            gourou::ByteArray rights(std::string(book.child_value("rights")));
            client.zipWriteFile(archive, "META-INF/rights.xml", rights);
            client.zipClose(archive);
        } else if (operation == 3) {
            processor.removeDRM(input, output, gourou::DRMProcessor::EPUB);
        } else throw std::runtime_error("Unknown native operation");
        return 0;
    } catch (const std::exception& e) {
        if (capacity) std::snprintf(error, capacity, "%s", e.what());
    } catch (...) {
        if (capacity) std::snprintf(error, capacity, "%s", "Unknown native backend error");
    }
    return -1;
}
