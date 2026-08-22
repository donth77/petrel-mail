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
  rows[8].tags = [{ name: 'Urgent', colour: '#A8544B' }];
  rows[9].tags = [{ name: 'Urgent', colour: '#A8544B' }];

  var folders = [
    { id: 101, role: '', path: 'Contracts' },
    { id: 102, role: '', path: 'Contracts/2026' },
    { id: 103, role: '', path: 'Client contact' },
    { id: 1, role: 'archive', path: 'Archive' },
  ];
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
      return {
        seeding: seeding,
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
      if (view === 'outbox') return [];
      if (view.indexOf('tag:') === 0) {
        var name = view.slice(4);
        return rows.filter(function (r) {
          return r.tags.some(function (t) { return t.name === name; });
        });
      }
      return rows.filter(function (r) { return r.filed === view; });
    },
    list_messages: function () {
      return rows;
    },
    search_messages: function () {
      return rows;
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
      return n > 0 ? [['inbox', n]] : [];
    },
    list_accounts: function () {
      return [
        { id: 1, kind: 'gmail', email: 'you@example.com', display_name: '',
          color: '#0E7C86', local_archive: 0, message_count: rows.length,
          unread_count: 0, last_sync_ms: now, folders: [] },
      ];
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
          attachments: [],
        },
      ];
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
            row.tags.push({ name: tg.name, colour: tg.colour });
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
    create_folder: function (a) {
      var id = 200 + folders.length;
      folders.push({ id: id, role: '', path: a.path });
      return id;
    },
    send_message: function (a) {
      // Records what the UI actually asked to send, so a test can assert on the
      // request rather than on what the composer rendered.
      if (!a.to || a.to.length === 0) throw 'no recipient';
      return 'test-' + Date.now() + '@example.com';
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
    quote_message: function () {
      return {
        html: '<p>Could you take a look before Friday?</p><p>Thanks,<br>Dana</p>',
        text: 'Could you take a look before Friday?\n\nThanks,\nDana',
        from: 'Dana Wu',
        date_ms: Date.now() - 3600000,
      };
    },
    popout_message: function () {
      // No second window in a browser; recording the call is the assertion.
      return null;
    },
    save_draft: function () { return 1; },
    load_draft: function () {
      return { id: 1, to: '', subject: '', body: '', html: '' };
    },
    delete_draft: function () { return null; },
    create_tag: function (a) {
      var id = 300 + tags.length;
      tags.push({ id: id, name: a.name, colour: '', thread_count: 0 });
      return id;
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
