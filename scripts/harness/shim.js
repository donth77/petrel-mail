/* A stand-in for Tauri's IPC layer.
 *
 * The point of this file is to run the *production* bundle — byte for byte the
 * code the desktop app ships — in a browser we can drive and inspect. The dev
 * server is not a substitute: it serves a different build against a different
 * api module, so a fault that only exists in the shipped bundle is invisible
 * there. That gap is exactly how a broken triage path got as far as a release.
 *
 * Every command the bundle invokes is recorded in window.__PETREL_IPC__, so a
 * test can assert on what the UI actually asked the backend to do, rather than
 * on what it rendered afterwards.
 */
(function () {
  window.__PETREL_IPC__ = [];
  var rules = [
    { id: 701, position: 0, enabled: true, name: 'Newsletters aside',
      conditions: [{ field: 'list_id', contains: 'news' }],
      actions: { move_to: 7, tag: null, mark_read: false, skip_inbox: true } },
  ];

  var now = Date.now();
  var rows = Array.from({ length: 40 }, function (_, i) {
    return {
      // Negative thread ids on purpose: real unthreaded mail is keyed by
      // coalesce(thread_id, -id), and a fixture with tidy positive ids would
      // hide sign bugs in anything that round-trips a thread id.
      thread_id: -(i + 1),
      id: i + 1,
      from_display: 'Sam Ortiz',
      from_addr: 'sam@example.com',
      subject: 'Conversation ' + (i + 1),
      snippet: 'body text here',
      date_ms: now - i * 60000,
      message_count: 1,
      participants: 'Sam Ortiz',
      unread: i % 3 === 0,
      starred: false,
      has_attachments: false,
      tags: [],
      attachment_name: '',
      // Where the engine would have filed it; '' means still in the inbox.
      filed: '',
      snoozed: 0,
    };
  });

  // A few rows start out filed and tagged, so the views that are not the inbox
  // have something in them. Without these, Sent, Drafts and the tag views were
  // all permanently empty here — and a bug that only shows up in those views
  // could not be reproduced, which is exactly what happened with trash.
  rows[5].filed = 'sent';
  rows[6].filed = 'sent';
  rows[7].filed = 'drafts';
  // One filed into a user folder, so the way back out is exercisable.
  rows[8].filed = 7;
  rows[8].tags = [{ id: 11, name: 'Urgent', colour: '#A8544B' }];
  rows[9].tags = [{ id: 11, name: 'Urgent', colour: '#A8544B' }];

  var folders = [
    { id: 101, role: '', path: 'Contracts' },
    { id: 102, role: '', path: 'Contracts/2026' },
    { id: 103, role: '', path: 'Client contact' },
    { id: 1, role: 'archive', path: 'Archive' },
    { id: 2, role: 'inbox', path: 'INBOX' },
    { id: 3, role: 'trash', path: 'Trash' },
        { id: 7, role: '', path: 'Receipts' },
        { id: 8, role: '', path: 'Projects/Petrel' },
        { id: 9, role: '', path: 'Archive/Old letters' },
        { id: 10, role: '', path: 'Archive/Old letters/2019' },
  ];
  // ?gmailFolders=1: the same rail over a Gmail-shaped account — roles wear
  // the reserved [Gmail] names, and user folders sit at the top level plus
  // one already archived under the plain `Archive` label the anchor uses.
  if (location.search.indexOf('gmailFolders=1') !== -1) {
    folders = [
      { id: 2, role: 'inbox', path: 'INBOX' },
      { id: 4, role: 'archive', path: '[Gmail]/All Mail' },
      { id: 10, role: 'trash', path: '[Gmail]/Trash' },
      { id: 8, role: 'spam', path: '[Gmail]/Spam' },
      { id: 120, role: '', path: 'Test-Folder' },
      { id: 121, role: '', path: 'Test-Folder/sub1' },
      { id: 122, role: '', path: 'Test-Folder/sub1/sub2' },
      { id: 130, role: '', path: 'Archive/Old letters' },
    ];
  }
  // ?ncFolders=1: a Namecheap-shaped account — no archive role at all, a
  // plain folder named Archive doing the job by convention, deleted folders
  // living under the Trash role, and a lowercase sibling that once looked
  // like the Archive tree's parent.
  if (location.search.indexOf('ncFolders=1') !== -1) {
    folders = [
      { id: 119, role: 'inbox', path: 'INBOX' },
      { id: 26, role: 'trash', path: 'Trash' },
      { id: 24, role: 'sent', path: 'Sent' },
      { id: 23, role: 'spam', path: 'Spam' },
      { id: 82, role: '', path: 'Archive' },
      { id: 84, role: '', path: 'Archive/Old letters' },
      { id: 21, role: '', path: 'apply' },
      { id: 120, role: '', path: 'Test-Folder' },
      { id: 121, role: '', path: 'Test-Folder/sub1' },
      { id: 47, role: '', path: 'Trash/binned' },
    ];
  }
  var tags = [
    { id: 11, name: 'Urgent', colour: '#A8544B', thread_count: 0 },
    { id: 12, name: 'Waiting on', colour: '#3B6EA5', thread_count: 0 },
    { id: 13, name: 'Receipts', colour: '#5E7C4A', thread_count: 0 },
  ];

  // Set window.__PETREL_WRITTEN_TO__ before load to exercise the auto-allow
  // path — someone the user has emailed is trusted without being in the list.
  var trustedSenders = [];
  var writtenTo = [];
  try {
    writtenTo = JSON.parse(localStorage.getItem('__petrel_written_to') || '[]');
  } catch (e) {}

  var identity = {
    address: 'you@example.com',
    display_name: 'You',
    signature: '',
    signature_on_reply: false,
  };

  var handlers = {
    status: function () {
      // Set localStorage.__petrel_seeding to model an active sync, which polls
      // status every 400ms instead of every 5s. That cadence is what exposed a
      // toast whose dismissal timer restarted on every render.
      var seeding = false;
      try { seeding = localStorage.getItem('__petrel_seeding') === '1'; } catch (e) {}
      // localStorage.__petrel_unconfigured models a first run, so the harness
      // can show onboarding without deleting anyone's account.
      var configured = true;
      try { configured = localStorage.getItem('__petrel_unconfigured') !== '1'; } catch (e) {}
      return {
        last_sync_ms: Date.now() - 3 * 60000,
        notify: (function () {
          if (location.search.indexOf('ruleNotify=1') === -1) return [];
          window.__STATUS_N__ = (window.__STATUS_N__ || 0) + 1;
          // Delivered on the third poll, once: by then the launch toasts
          // have had their say and the probe can see this one standing.
          if (window.__STATUS_N__ === 3 && !window.__NOTIFIED__) {
            window.__NOTIFIED__ = true;
            return [['Robo Recruiter', 'Your application was received']];
          }
          return [];
        })(),
        configured: configured,
        seeding: seeding,
        // A denominator larger than what is held, so the coverage line has
        // something true to say during a backfill.
        server_total: 120,
        count: rows.length,
        source: 'test@example.com',
        retention: '',
        data_dir: '/tmp',
        // Flip to a string to exercise the sync-failure banner.
        // Set localStorage.__petrel_sync_error to exercise the failure banner.
        sync_error: (function () {
          try { return localStorage.getItem('__petrel_sync_error') || null; } catch (e) { return null; }
        })(),
      };
    },
    list_threads: function (a) {
      // Views are modelled here, not faked away. A shim that returns the same
      // rows for every view would let an unimplemented view look implemented,
      // which is the exact failure this harness exists to catch.
      var view = a.view || 'inbox';
      var now = Date.now();
      if (view === 'inbox') {
        return rows.filter(function (r) { return !r.filed && !(r.snoozed > now); });
      }
      if (view === 'snoozed') return rows.filter(function (r) { return r.snoozed > now; });
      if (view === 'starred') return rows.filter(function (r) { return r.starred; });
      // User folders: what the move test filed lands here, so the way back
      // out of a folder can be exercised too.
      if (view.indexOf('folder:') === 0) {
        var fid = Number(view.slice(7));
        return rows.filter(function (r) { return r.filed === fid; });
      }
      if (view === 'outbox') return [];
      if (view.indexOf('tag:') === 0) {
        var name = view.slice(4);
        return rows.filter(function (r) {
          return r.tags.some(function (t) { return t.name === name; });
        });
      }
      return rows.filter(function (r) { return r.filed === view; });
    },
    search_messages: function (a) {
      // Modelled, not stubbed: results have to carry why they matched, and the
      // ordering has to change when the sort does — a shim that returned the
      // inbox would let both look right while doing nothing.
      var q = (a.query || '').toLowerCase();
      var found = rows.filter(function (r) {
        return !r.filed && (r.subject + ' ' + r.snippet).toLowerCase().indexOf(q) >= 0;
      }).map(function (r) {
        return Object.assign({}, r, {
          // The engine's markers, not brackets — see the Snippet renderer.
          match_snippet:
            '…the revised \u{E000}' + (a.query || '') + '\u{E001} and the pricing sheet…',
        });
      });
      if (a.newest) found.sort(function (x, y) { return y.date_ms - x.date_ms; });
      return found;
    },
    list_tags: function () {
      return tags;
    },
    view_counts: function (a) {
      // Modelled, not faked: a shim that always returned the same numbers
      // would let a badge that ignores its setting look correct.
      if (a.mode === 'off') return [];
      var now = Date.now();
      var live = rows.filter(function (r) { return !r.filed && !(r.snoozed > now); });
      var n = a.mode === 'total'
        ? live.length
        : live.filter(function (r) { return r.unread; }).length;
      var out = n > 0 ? [['inbox', n]] : [];
      // The outbox's own numbers: how many are held, and how many of those
      // are waiting on a person — the one that turns the rail amber.
      out.push(['outbox', 5]);
      out.push(['outbox:attention', 1]);
      return out;
    },
    list_accounts: function () {
      // Two accounts, so switching can be exercised. Which is active is
      // remembered across calls the way the store remembers it.
      var active = 1;
      try { active = Number(localStorage.getItem('__petrel_active_account') || 1); } catch (e) {}
      return [
        { id: 1, kind: 'gmail', email: 'you@example.com', display_name: '',
          color: '#0E7C86', local_archive: 0, active: active === 1, message_count: rows.length,
          unread_count: 0, last_sync_ms: now, folders: [] },
        { id: 2, kind: 'imap', email: 'tom@northbay.example', display_name: '',
          color: '#9A6B1F', local_archive: 0, active: active === 2, message_count: 41,
          unread_count: 3, last_sync_ms: now, folders: [] },
      ];
    },
    set_active_account: function (a) {
      try { localStorage.setItem('__petrel_active_account', String(a.accountId)); } catch (e) {}
      return null;
    },
    get_settings: function () {
      return {};
    },
    set_setting: function () {
      return null;
    },
    set_account_color: function () {
      return null;
    },
    set_account_archive: function () {
      return null;
    },
    thread_detail: function () {
      return [
        {
          id: 1,
          from_display: 'Sam Ortiz',
          from_addr: 'sam@example.com',
          to_display: 'me',
          date_ms: now,
          subject: 'Conversation 1',
          recipients: '',
          // Two files, one previewable and one not, so both verbs and the
          // executable warning can be exercised in the browser.
          attachments: [
            { filename: 'diagram.png', size: 48123, part: 0, mime: 'image/png' },
            { filename: 'setup.sh', size: 1290, part: 1, mime: 'text/x-shellscript' },
          ],
          has_calendar: true,
          invite_response: null,
        },
      ];
    },
    invitation: function (a) {
      // Message 1 wears a live REQUEST; ask with ?invCancel=1 for the
      // cancellation face of the card.
      var cancelled = location.search.indexOf('invCancel=1') !== -1;
      return {
        method: cancelled ? 'CANCEL' : 'REQUEST',
        summary: 'Planning 1:1',
        location: 'Video call',
        description: 'Bring the draft.',
        organizer_name: 'Dana Wu',
        organizer_email: 'dana@example.com',
        attendees: [
          { name: 'me', email: 'tom@northbay.example', partstat: 'NEEDS-ACTION' },
        ],
        start: { kind: 'utc', ms: Date.now() + 3 * 86400000 },
        end: { kind: 'utc', ms: Date.now() + 3 * 86400000 + 3600000 },
        recurring: false,
        status: cancelled ? 'CANCELLED' : 'CONFIRMED',
        my_partstat: location.search.indexOf('invAccepted=1') !== -1 ? 'ACCEPTED' : 'NEEDS-ACTION',
        can_respond: !cancelled,
        responded: null,
      };
    },
    respond_invitation: function () { return null; },
    draft_conflict: function () {
      return location.search.indexOf('draftConflict=1') !== -1 && !window.__CONFLICT_DONE__
        ? { other_id: 909 }
        : null;
    },
    resolve_draft_conflict: function () { window.__CONFLICT_DONE__ = true; return null; },
    // The browser cannot hand a URL to the system, and navigating the
    // harness tab away would take the app under test with it.
    open_external: function () { return null; },
    empty_trash: function () { return '7/0'; },
    check_update: function () {
      // ?update=1 offers one; ?update=err makes the check itself fail, which
      // must not read as "up to date".
      if (location.search.indexOf('update=err') !== -1) {
        return { current: '0.0.1', available: null, notes: null, error: 'network unreachable' };
      }
      if (location.search.indexOf('update=1') !== -1) {
        return { current: '0.0.1', available: '0.2.0', notes: 'Faster lists, invitations.', error: null };
      }
      return { current: '0.0.1', available: null, notes: null, error: null };
    },
    install_update: function () { return null; },
    restart_for_update: function () { return null; },
    load_draft: function (a) {
      return window.__CONFLICT_DONE__
        ? { id: a.id, to: 'sam@example.com', subject: 'plans, revised', body: 'second thoughts', html: '' }
        : { id: a.id, to: 'sam@example.com', subject: 'plans', body: 'first words', html: '' };
    },
    attachment_is_executable: function (a) {
      return /\.(exe|bat|sh|js|jar|dmg|app|py)$/i.test(String(a.filename || ''));
    },
    save_attachment: function () { return null; },
    open_attachment: function () { return null; },
    attachment_url: function (a) {
      // A real image URL, so the preview frame shows something: a data-URI
      // cannot cross the shim boundary, so the stand-in page serves one.
      return './msg.html?attachment=' + a.messageId + '-' + a.part;
    },
    message_url: function () {
      // A stand-in frame, so the reading pane actually has one. Set
      // window.__PETREL_BLOCKED__ to model a message with remote content in it.
      // localStorage rather than a window global: setting it means reloading,
      // and a reload wipes the global before the app can read it.
      var n = 0;
      try { n = Number(localStorage.getItem('__petrel_blocked') || 0); } catch (e) {}
      var extra = '';
      try {
        extra = localStorage.getItem('__petrel_body') || '';
      } catch (e) {}
      return './msg.html?blocked=' + n + (extra ? '&' + extra : '');
    },
    // Remote content, modelled rather than stubbed: the policy is the point of
    // the feature, so a shim that always answered "allowed" would let a banner
    // that ignores it look correct.
    remote_status: function () {
      var addr = 'sam@example.com';
      return {
        from_addr: addr,
        allowed: trustedSenders.indexOf(addr) >= 0 || writtenTo.indexOf(addr) >= 0,
        because_written_to:
          writtenTo.indexOf(addr) >= 0 && trustedSenders.indexOf(addr) < 0,
      };
    },
    show_remote_once: function () {
      return null;
    },
    trust_sender: function () {
      var addr = 'sam@example.com';
      if (trustedSenders.indexOf(addr) < 0) trustedSenders.push(addr);
      return addr;
    },
    trusted_senders: function () {
      return trustedSenders.slice();
    },
    untrust_sender: function (a) {
      var i = trustedSenders.indexOf(a.addr);
      if (i >= 0) trustedSenders.splice(i, 1);
      return null;
    },
    frontend_log: function () {
      return null;
    },
    triage: function (a) {
      // Move the fixture the way the engine would, so a view switch after a
      // triage action shows what the real one would show.
      var row = rows.filter(function (r) { return r.thread_id === a.threadId; })[0];
      if (row) {
        if (a.kind === 'archive') row.filed = 'archive';
        else if (a.kind === 'trash') row.filed = 'trash';
        else if (a.kind === 'spam') row.filed = 'spam';
        else if (a.kind === 'star') row.starred = true;
        else if (a.kind === 'unstar') row.starred = false;
        else if (a.kind === 'mark_read') row.unread = false;
        else if (a.kind === 'mark_unread') row.unread = true;
        else if (a.kind === 'move') row.filed = 'moved';
        else if (a.kind === 'delete_forever') rows.splice(rows.indexOf(row), 1);
        else if (a.kind === 'snooze') row.snoozed = a.target;
        else if (a.kind === 'unsnooze') row.snoozed = 0;
        else if (a.kind === 'tag') {
          var tg = tags.filter(function (x) { return x.id === a.target; })[0];
          if (tg && !row.tags.some(function (x) { return x.name === tg.name; })) {
            row.tags.push({ id: tg.id, name: tg.name, colour: tg.colour });
          }
        } else if (a.kind === 'untag') {
          var t2 = tags.filter(function (x) { return x.id === a.target; })[0];
          if (t2) row.tags = row.tags.filter(function (x) { return x.name !== t2.name; });
        }
      }
      var past = {
        archive: 'Archived',
        trash: 'Moved to trash',
        spam: 'Marked as spam',
        star: 'Starred',
        unstar: 'Unstarred',
        mark_read: 'Marked read',
        mark_unread: 'Marked unread',
        move: 'Moved',
        tag: 'Tagged',
        untag: 'Untagged',
        snooze: 'Snoozed',
        unsnooze: 'Back in the inbox',
        delete_forever: 'Deleted',
      };
      return {
        action_id: window.__PETREL_IPC__.length,
        kind: a.kind,
        message_count: 1,
        description: past[a.kind] || a.kind,
      };
    },
    undo_triage: function () {
      return true;
    },
    list_folders: function () {
      return folders;
    },
    rename_folder: function () { return null; },
    push_draft: function () { return null; },
    import_mail: function () { return { imported: 3, duplicates: 1, failed: 0 }; },
    print_message: function () { return null; },
    view_count: function () { return 40; },
    list_rules: function () { return rules.slice(); },
    save_rule: function (a) {
      if (a.ruleId) {
        for (var i = 0; i < rules.length; i++) {
          if (rules[i].id === a.ruleId) {
            rules[i] = { id: a.ruleId, position: rules[i].position, enabled: a.enabled, name: a.name, conditions: a.conditions, actions: a.actions };
          }
        }
        return a.ruleId;
      }
      var id = 700 + rules.length;
      rules.push({ id: id, position: rules.length, enabled: a.enabled, name: a.name, conditions: a.conditions, actions: a.actions });
      return id;
    },
    delete_rule: function (a) {
      rules = rules.filter(function (r) { return r.id !== a.ruleId; });
      return null;
    },
    move_rule: function () { return null; },
    unsubscribe_info: function (a) {
      // The newsletter stand-in offers one-click; everything else offers none.
      return a.messageId === 1
        ? { one_click: true, url: 'https://news.example/u/1', mailto: null }
        : null;
    },
    unsubscribe_one_click: function () { return null; },
    delete_folder: function () { return null; },
    create_folder: function (a) {
      var id = 200 + folders.length;
      folders.push({ id: id, role: '', path: a.path });
      return id;
    },
    // The dialog plugin's commands, so the attach and export flows can be
    // driven without a real file panel. Set window.__PETREL_PICK__ to the
    // paths a pick should return, or null to simulate cancelling.
    'plugin:dialog|open': function () {
      return window.__PETREL_PICK__ === undefined ? null : window.__PETREL_PICK__;
    },
    'plugin:dialog|save': function () {
      return window.__PETREL_SAVE__ === undefined ? null : window.__PETREL_SAVE__;
    },
    attachment_info: function (a) {
      // Sizes chosen to exercise the limit: the second file alone is fine and
      // the two together are not.
      return (a.paths || []).map(function (path, i) {
        return {
          path: path,
          name: path.split('/').pop() || path,
          size: i === 0 ? 1024 : 15 * 1024 * 1024,
        };
      });
    },
    get_identity: function () {
      return identity;
    },
    set_identity: function (a) {
      identity.display_name = a.displayName;
      identity.signature = a.signature;
      identity.signature_on_reply = a.signatureOnReply;
      return null;
    },
    storage_report: function () {
      return {
        messages: rows.length, attachments: 2,
        database_bytes: 12582912, blob_bytes: 41943040, index_bytes: 3145728,
        accounts: [
          { account_id: 1, messages: rows.length, blob_bytes: 33554432 },
          { account_id: 2, messages: 0, blob_bytes: 8388608 },
        ],
      };
    },
    export_mbox: function () {
      return rows.length + '/0';
    },
    complete_addresses: function (a) {
      var people = [
        { addr: 'nadia@example.com', display: 'Nadia Okafor', written_to: true },
        { addr: 'news@example.com', display: 'News Digest', written_to: false },
        { addr: 'nathan@other.example', display: '', written_to: false },
      ];
      var q = (a.prefix || '').toLowerCase();
      if (!q) return [];
      return people.filter(function (p) {
        return p.addr.indexOf(q) === 0 || p.display.toLowerCase().indexOf(q) === 0;
      });
    },
    // No filesystem in a browser, so the staged path is a stand-in. The point
    // is that the command exists and answers with the shape the composer
    // expects — a missing one made a dropped file look like a broken feature
    // when only the harness was missing.
    stage_attachment: function (args) {
      var name = (args && args.name) || 'attachment';
      var bytes = (args && args.bytes) || [];
      return { path: '/staged/' + name, name: name, size: bytes.length || 0 };
    },
    // The outbox, one row per state, so all five designs can be seen at
    // once without staging five real failures.
    discover_account: function (a) {
      var addr = String(a.address || '');
      var domain = addr.split('@')[1] || '';
      if (domain === 'gmail.com') {
        return { provider: 'Gmail', via: 'known-provider',
          imap: { host: 'imap.gmail.com', port: 993, tls: true },
          smtp: { host: 'smtp.gmail.com', port: 465, tls: true },
          auth: 'app-password', app_password_url: 'https://myaccount.google.com/apppasswords' };
      }
      if (domain === 'northbay.example') {
        return { provider: 'Namecheap Private Email', via: 'mx',
          imap: { host: 'mail.privateemail.com', port: 993, tls: true },
          smtp: { host: 'mail.privateemail.com', port: 465, tls: true },
          auth: 'password', app_password_url: null };
      }
      return null;
    },
    guess_servers: function (a) {
      var domain = String(a.address || '').split('@')[1] || 'example';
      return [{ host: 'imap.' + domain, port: 993, tls: true }, { host: 'smtp.' + domain, port: 465, tls: true }];
    },
    test_account: function (a) {
      if (a.setup && a.setup.password === 'wrong') throw new Error('Incoming (IMAP) — [AUTHENTICATIONFAILED] Invalid credentials');
      return null;
    },
    add_account: function () {
      try { localStorage.removeItem('__petrel_unconfigured'); } catch (e) {}
      return 2;
    },
    remove_account: function () { return null; },
    list_outbox: function () {
      var now = Date.now();
      return [
        { id: 901, subject: 'Re: Q3 vendor contracts — pricing before Friday', to: 'Sam Ortiz, Dana Wu',
          send_after_ms: now + 7000, state: 'RetryQueued', error: null, attempts: 0, next_ms: null, attachments: 0 },
        { id: 902, subject: 'Invoice 2214', to: 'accounts@clientco.example',
          send_after_ms: now - 60000, state: 'RetryQueued', error: 'connect: network unreachable',
          attempts: 1, next_ms: now + 30000, attachments: 1 },
        { id: 903, subject: 'Notes from Tuesday', to: 'maya@northbay.example',
          send_after_ms: now - 120000, state: 'RetryQueued', error: 'connect: connection refused',
          attempts: 2, next_ms: now + 120000, attachments: 0 },
        { id: 904, subject: 'Board pack v4', to: 'directors@northbay.example',
          send_after_ms: now - 300000, state: 'NeedsAttention', error: 'connection closed after DATA',
          attempts: 1, next_ms: null, attachments: 2 },
        { id: 905, subject: 'Welcome aboard!', to: 'j.smith@oldcompany.example',
          send_after_ms: now - 400000, state: 'FailedPermanent', error: '550 — no such user here',
          attempts: 1, next_ms: null, attachments: 0 },
      ];
    },
    outbox_send_now: function () { return null; },
    outbox_edit: function () { return null; },
    outbox_check: function () { return 'NeedsAttention'; },
    quote_message: function () {
      return {
        html: '<p>Could you take a look before Friday?</p><p>Thanks,<br>Dana</p>',
        text: 'Could you take a look before Friday?\n\nThanks,\nDana',
        from: 'Dana Wu',
        date_ms: Date.now() - 3600000,
        // A forward's header block needs the message's own recipients and
        // subject. Returning the reply's shape here is what let a forward that
        // reads them ship as working — the stand-in has to match the command.
        to: 'Sam Ortiz <sam@example.com>, you@example.com',
        subject: 'Q3 vendor contracts',
      };
    },
    popout_message: function () {
      // No second window in a browser; recording the call is the assertion.
      return null;
    },
    save_draft: function () { return 1; },
    delete_draft: function () { return null; },
    create_tag: function (a) {
      var id = 300 + tags.length;
      tags.push({ id: id, name: a.name, colour: '', thread_count: 0 });
      return id;
    },
    // Editing a tag, so the harness exercises renaming, colouring and deleting
    // rather than only creating — the three that had no way in at all.
    rename_tag: function (a) {
      var t = tags.find(function (x) { return x.id === a.tagId; });
      if (!t) return null;
      var clash = tags.some(function (x) {
        return x.id !== a.tagId && x.name.toLowerCase() === String(a.name).trim().toLowerCase();
      });
      if (clash) throw new Error('a tag called ' + a.name + ' already exists');
      t.name = String(a.name).trim();
      return null;
    },
    set_tag_colour: function (a) {
      var t = tags.find(function (x) { return x.id === a.tagId; });
      if (t) t.colour = a.colour;
      return null;
    },
    delete_tag: function (a) {
      var i = tags.findIndex(function (x) { return x.id === a.tagId; });
      if (i >= 0) tags.splice(i, 1);
      return null;
    },
  };

  window.__TAURI_INTERNALS__ = {
    invoke: function (cmd, args) {
      window.__PETREL_IPC__.push({ cmd: cmd, args: args });
      var h = handlers[cmd];
      // An unregistered command rejects rather than resolving to undefined:
      // silently succeeding would let a renamed command pass unnoticed.
      if (!h) return Promise.reject('no such command: ' + cmd);
      try {
        return Promise.resolve(h(args || {}));
      } catch (e) {
        return Promise.reject(String(e));
      }
    },
    transformCallback: function (cb) {
      var id = Math.floor(Math.random() * 1e9);
      window['_' + id] = cb;
      return id;
    },
  };
})();
