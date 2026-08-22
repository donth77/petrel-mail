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
    };
  });

  var handlers = {
    status: function () {
      return {
        seeding: false,
        count: rows.length,
        source: 'test@example.com',
        retention: '',
        data_dir: '/tmp',
      };
    },
    list_threads: function (a) {
      // Views are modelled here, not faked away. A shim that returns the same
      // rows for every view would let an unimplemented view look implemented,
      // which is the exact failure this harness exists to catch.
      var view = a.view || 'inbox';
      if (view === 'inbox') return rows.filter(function (r) { return !r.filed; });
      if (view === 'starred') return rows.filter(function (r) { return r.starred; });
      if (view === 'snoozed' || view === 'outbox') return [];
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
      return [];
    },
    list_accounts: function () {
      return [];
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
      return '';
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
      }
      var past = {
        archive: 'Archived',
        trash: 'Moved to trash',
        spam: 'Marked as spam',
        star: 'Starred',
        unstar: 'Unstarred',
        mark_read: 'Marked read',
        mark_unread: 'Marked unread',
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
