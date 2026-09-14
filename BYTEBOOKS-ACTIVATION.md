# Direct ByteBooks activation

Investigation: 2026-09-14. Recommendation: extend the bundled libgourou ADEPT
activation flow. Keep account creation/migration in the browser and retain
`adobe setup` activation ZIP import. Direct activation is not implemented yet.

## What is established

[Adobe's transition FAQ](https://helpx.adobe.com/captivate/kb/eol-faq-adobe-digital-editions.html),
updated August 27, 2026, says users sign in with ByteBooks credentials wherever
Digital Editions references Adobe ID. Existing users should create their
ByteBooks account with the same email to retain their license association.
Already-authorized installations need no immediate action. Account setup is
described on the [ByteBooks transition page](https://dtsbytebooks.com/transition-help).

This supports investigating the existing protocol; it does not prove that
Crossload's library can successfully activate against the current service.
The ByteBooks FAQ returned HTTP 403 to the research browser, and the transition
page supplied no readable body. No public third-party OAuth/device-code contract
was established in this investigation.

## Native implementation available

The vendored `native/vendor/libgourou/include/libgourou.h` exposes
`createDRMProcessor`, `signIn`, and `activateDevice`. The implementation in
`src/user.cpp` and `src/libgourou.cpp` follows this sequence:

1. Generate a device identity, device salt and activation environment.
2. Fetch ActivationServiceInfo and AuthenticationServiceInfo.
3. Build an encrypted credential request using the advertised authentication
   certificate and submit SignInDirect (method AdobeID by default).
4. Save returned credentials and keys; sign an Activate request.
5. Save the activation token in activation.xml.

Crossload's `native/bridge.cpp` currently requires an existing activation and
exposes validation, fulfillment, download and decryption only. A separate
initialization entry point is needed. Account registration and email verification
are not implemented by these native functions.

## Live read-only checks

HTTPS GET requests to these public endpoints succeeded on September 14:

- https://adeactivate.adobe.com/adept/ActivationServiceInfo
- https://adeactivate.adobe.com/adept/AuthenticationServiceInfo

The first returned ADEPT activationServiceInfo with authURL and userInfoURL set
to `http://adeactivate.adobe.com/adept`. Authentication metadata was also available
through HTTPS. No credentials, sign-in POSTs or activation requests were sent.

The library follows the advertised authURL. Its transport currently permits
HTTP and follows redirects, so exposing signIn unchanged is unsuitable. Enforce
HTTPS for the known service before requesting authentication metadata or sending
credentials; reject unexpected service origins and credential redirects. Do not
change arbitrary book-download URLs as part of this activation-specific work.

## Proposed user flow (not yet available)

`crossload adobe activate` prompts interactively for ByteBooks email and a hidden
password. Account setup or migration stays on the official website. Passwords
must not be passed as command arguments, saved in configuration, or logged.
Avoid anonymous activation, which does not provide the intended reusable account
workflow. Existing activation data must remain intact.

## Implementation sequence

1. Add a native activation entry point and HTTPS-only service policy. Suppress
   protocol logs and sanitize server errors so credentials and keys cannot leak.
2. Create private staging state (directory 0700, files 0600), with an explicit
   state lock. Preserve a single device identity and completed phases across
   interruption. Do not automatically replay an ambiguous Activate POST or
   generate a new device on every retry: registration may consume account slots.
3. Add interactive secret input and phase-specific errors for account migration,
   rejected credentials, activation limits and uncertain server completion.
   Keep the existing activation untouched; reuse its validation before publishing
   all three activation files atomically as one environment.
4. Test against synthetic servers: success, wrong credentials, malformed replies,
   insecure service discovery, redirects, interruption, existing-state protection,
   permissions and secret redaction. Run native Linux and macOS CI.
5. Verify one real account activation from the user's terminal, then import a
   book and export/reimport its activation. The user enters credentials locally;
   no credentials should be supplied through chat. Device registration is a
   separate real-account action from this completed read-only investigation.

Until that verification, ZIP import remains the supported activation method.
