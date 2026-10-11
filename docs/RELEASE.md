# Release

Filled by Phase 15, except the policy notes below which constrain earlier phases.

## Desktop distribution

Direct download: signed and notarized macOS universal build, signed Windows
installer. No Mac App Store, no Microsoft Store.

- macOS: `universal-apple-darwin` target, `codesign` + `notarytool`, hardened
  runtime, minimum macOS 11.0.
- Windows: `nsis` installer, `perMachine` install mode, code-signed.
- Updater: arrives with the release pipeline, because it needs a signing keypair
  and an HTTPS endpoint for signed manifests. It is deliberately not registered
  before then. The plugin panics at startup if its config is absent, and an
  updater that cannot check for updates is a fake capability.

## Mobile distribution

TestFlight / App Store for iOS, Play internal testing / Google Play for Android.
A direct-APK download from the website is an allowed alternative Android channel.
Mobile updates ship through the stores; the Tauri updater does not support them.

## Store policy risk (read before Phase 13/14)

Billing is QRIS only, and **the iOS and Android apps do not sell anything**: no
checkout, no purchase buttons, no prices, and no links or instructions pointing
to a website purchase. Users buy on the web or the desktop app, and the plan then
syncs to their account on every device.

This is enforced in three places rather than by convention:

1. `config/billing_policy.toml`. `validate()` rejects a mobile entry that
   enables purchase UI, prices, or an upgrade CTA, so the file cannot be edited
   into a violation.
2. `GET /plans` omits prices on those platforms.
3. `POST /billing/checkout` is rejected for `platform in (ios, android)`.

**The risk to document and accept:** both Apple and Google require their own
in-app purchase for digital subscriptions in many regions. A QRIS-only mobile app
that unlocks paid features may be rejected, or required to add store billing. The
`PaymentProvider` trait is kept generic so store billing can be added later
without restructuring. Confirm the current rules for Indonesia before submission
rather than relying on this note.

## Privacy labels

Store privacy disclosures must be confirmed before launch. Known data the app
handles: account email, uploaded study material, chat content, calendar events,
device identifiers for push, and payment records. B2 in `docs/ASSUMPTIONS.md`.

## Account deletion

In-app `Delete account`, also required by app store rules. It deletes or
anonymizes the user's data, revokes all sessions, and cancels pending jobs. There
are no recurring charges to cancel; remaining prepaid time is forfeited, which
must be stated in the UI before confirmation.

`Export my data` produces JSON + Markdown + ICS.

## Tax and PPN

Unconfirmed (B2). Invoices currently carry no tax line items. Tax treatment and
PPN handling must be settled before launch.

## Versioning

`CARGO_PKG_VERSION` for the server and shell, the same value in
`tauri.conf.json`. Mobile build numbers are bumped per store submission.
