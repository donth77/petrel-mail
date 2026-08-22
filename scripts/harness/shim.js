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
    list_threads: function () {
      return rows;
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
