# Native Telegram shared contacts and places

The authenticated native Telegram source accepts contact, location and venue
messages as bounded display text. This requires no file download or additional
operator flag. Private and group messages continue through the existing sender,
chat and addressing policy. Forum-topic identity remains unchanged. Channel posts
still require their explicit source opt-in and an admission policy permitting
unknown channel actors.

A contact includes its first name, optional last name and bounded phone number.
The contact's Telegram user ID is not the authenticated sender. A vCard is not
fetched, executed or converted into a resource reference. A location includes
finite, range-checked latitude and longitude plus optional horizontal accuracy.
A live location is labeled as a snapshot; no continuous tracking or live-edit
subscription is implied. A venue adds its title and address. Provider place IDs
do not become destinations or URLs. Display strings are bounded and quoted;
control characters and line breaks in labels are rejected. Quotes cannot
impersonate the formatter's field labels.

Ambiguous contact/location/media mixtures and malformed facts are refused as
unsupported content without fabricating text. Telegram's compatibility location
field on a venue is accepted only when it equals the venue's own location.
These messages do not acquire attachment-download, contact-lookup or recipient
authority. The existing NyxID relay contract is unchanged.

Regression coverage exercises the native observer with inbound media disabled,
authenticated sender preservation, group-topic preservation, bot refusal,
channel opt-in, legacy relay refusal, bounds, malformed coordinates and escaped
display text. No external Telegram messages were sent during these tests.
