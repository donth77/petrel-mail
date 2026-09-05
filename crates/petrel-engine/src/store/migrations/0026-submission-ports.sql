-- Submission ports for the three providers that never offered implicit TLS.
--
-- iCloud and Outlook were set up on 465 because the provider table said so,
-- and nothing listens there: every send failed at connect. The table now
-- says 587, which is STARTTLS, but an account added before that keeps the
-- port it was created with — so the accounts already on disk are moved here.
--
-- Only where the port is still the one the old table handed out. A port the
-- person typed themselves is theirs, whatever it is. Server settings live in
-- the account's settings JSON, so this edits that rather than a column.
--
-- json_valid first: json_extract raises on a row that is not JSON, and one
-- such row would roll this back and stop the store opening at all. A row
-- the app cannot read is left exactly as it is.
UPDATE accounts
   SET settings_json = json_set(settings_json, '$.smtp_port', 587)
 WHERE json_valid(settings_json)
   AND json_extract(settings_json, '$.smtp_port') = 465
   AND lower(json_extract(settings_json, '$.smtp_host')) IN (
       'smtp.mail.me.com',
       'smtp-mail.outlook.com',
       'smtp.office365.com'
   );
