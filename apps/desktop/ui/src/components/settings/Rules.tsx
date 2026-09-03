import { useEffect, useState } from 'react';
import { ArrowDown, ArrowUp, Plus, Trash2 } from 'lucide-react';
import { api, type Folder, type Rule, type Tag } from '../../lib/api';
import { Icon } from '../Icon';
import { PickerField } from '../PickerField';
import { AccountNote } from './AccountNote';
import { filableFolderRows } from '../../lib/folders';
import { t } from '../../lib/strings';
import {
  FIELD_LABEL, OP_LABEL, RULE_FIELDS, opForField, opsFor, valueKind,
  type RuleField,
} from '../../lib/rules';

/** A rule summarised in one readable line, so the list explains itself. */
export function summary(rule: Rule, folders: Folder[], tags: Tag[]): string {
  const conds = rule.conditions
    .map((c) => {
      // The header's own name is what the rule is about, so it stands in for
      // the field: "X-Spam-Flag is YES" rather than "Header is YES".
      const what = c.field === 'header' && c.header ? c.header : t(FIELD_LABEL[c.field]);
      return `${what} ${t(OP_LABEL[c.op])} “${c.value}”`;
    })
    .join(' + ');
  const acts: string[] = [];
  const a = rule.actions;
  if (a.move_to != null) {
    const f = folders.find((x) => x.id === a.move_to);
    acts.push(f ? t('rule-sum-move', { folder: f.path }) : t('rule-sum-gone'));
  }
  // Only when there is nowhere to go: a named destination already takes the
  // mail out of the inbox, and the engine drops the archive rather than let
  // it undo the move. Saying both here described something it does not do.
  if (a.skip_inbox && a.move_to == null) acts.push(t('rule-sum-skip'));
  if (a.tag != null) {
    const tg = tags.find((x) => x.id === a.tag);
    acts.push(tg ? t('rule-sum-tag', { tag: tg.name }) : t('rule-sum-gone'));
  }
  if (a.mark_read) acts.push(t('rule-sum-read'));
  if (a.notify) acts.push(t('rule-sum-notify'));
  return `${conds} → ${acts.join(', ') || t('rule-sum-nothing')}`;
}

/**
 * Filter rules: triage written down once.
 *
 * The list is the truth about order — rules run top to bottom, every enabled
 * one that matches — so order is edited right here with arrows rather than
 * hidden behind a number field.
 */
export function Rules({ onMessage }: { onMessage: (text: string) => void }) {
  const [rules, setRules] = useState<Rule[]>([]);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [editing, setEditing] = useState<Rule | null>(null);

  const reload = () => {
    void api.listRules().then(setRules).catch((e) => onMessage(String(e)));
  };
  useEffect(() => {
    reload();
    void api.folders().then(setFolders).catch(() => {});
    void api.tags().then(setTags).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const blank: Rule = {
    id: 0,
    position: rules.length,
    enabled: true,
    name: '',
    conditions: [{ field: 'from', op: 'contains', value: '' }],
    actions: { move_to: null, tag: null, mark_read: false, skip_inbox: false, notify: false },
  };

  const save = async (r: Rule) => {
    // A condition needs a value, and a header condition needs to say which
    // header. Without that second test a rule saved with the header name left
    // blank matches nothing at all — it sits in the list looking enabled and
    // quietly never fires, which is the failure this editor exists to prevent.
    const conditions = r.conditions.filter(
      (c) => c.value.trim() && (c.field !== 'header' || (c.header ?? '').trim()),
    );
    if (!r.name.trim() || conditions.length === 0) {
      onMessage(t('rule-needs-substance'));
      return;
    }
    try {
      await api.saveRule(r.id || null, r.name.trim(), r.enabled, conditions, r.actions);
      setEditing(null);
      reload();
      onMessage(t('rule-saved', { name: r.name.trim() }));
    } catch (e) {
      onMessage(t('rule-failed', { error: String(e) }));
    }
  };

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-rules')}</h1>
      <AccountNote />

      <section className="field">
        <div className="flabel">{t('rules-on-arrival')}</div>
        <p className="fhelp">{t('rules-help')}</p>

        {rules.map((r, i) => (
          <div className="rule-row" key={r.id}>
            <input
              type="checkbox"
              checked={r.enabled}
              aria-label={t('rule-enabled')}
              onChange={(e) =>
                void api
                  .saveRule(r.id, r.name, e.target.checked, r.conditions, r.actions)
                  .then(reload)
                  .catch((err) => onMessage(t('rule-failed', { error: String(err) })))
              }
            />
            <button type="button" className="rule-name" onClick={() => setEditing(r)}>
              <span>{r.name}</span>
              <span className="rule-sum">{summary(r, folders, tags)}</span>
            </button>
            <button
              type="button" className="act-icon" aria-label={t('rule-up')}
              disabled={i === 0}
              onClick={() =>
                void api
                  .moveRule(r.id, true)
                  .then(reload)
                  .catch((err) => onMessage(t('rule-failed', { error: String(err) })))
              }
            >
              <Icon icon={ArrowUp} size={13} />
            </button>
            <button
              type="button" className="act-icon" aria-label={t('rule-down')}
              disabled={i === rules.length - 1}
              onClick={() =>
                void api
                  .moveRule(r.id, false)
                  .then(reload)
                  .catch((err) => onMessage(t('rule-failed', { error: String(err) })))
              }
            >
              <Icon icon={ArrowDown} size={13} />
            </button>
            <button
              type="button" className="act-icon" aria-label={t('rule-delete')}
              onClick={() =>
                void api
                  .deleteRule(r.id)
                  .then(reload)
                  .catch((err) => onMessage(t('rule-failed', { error: String(err) })))
              }
            >
              <Icon icon={Trash2} size={13} />
            </button>
          </div>
        ))}

        {!editing && (
          <button type="button" className="fbtn" onClick={() => setEditing(blank)}>
            <Icon icon={Plus} size={13} />
            {t('rule-new')}
          </button>
        )}
      </section>

      {editing && (
        <section className="field rule-editor">
          <input
            className="text-input"
            placeholder={t('rule-name-placeholder')}
            aria-label={t('rule-name-placeholder')}
            value={editing.name}
            onChange={(e) => setEditing({ ...editing, name: e.target.value })}
            onKeyDown={(e) => e.stopPropagation()}
          />

          <div className="sublabel">{t('rule-when')}</div>
          {editing.conditions.map((c, i) => (
            // Conditions have no identity of their own; position is it.
            <div className="rule-cond" key={i}>
              <select
                className="select"
                value={c.field}
                aria-label={t('rule-field')}
                onChange={(e) => {
                  const field = e.target.value as RuleField;
                  const conditions = [...editing.conditions];
                  // The operator follows the field. Leaving "contains"
                  // selected against Size would save a rule that can never
                  // match, and look configured while doing it.
                  conditions[i] = { ...c, field, op: opForField(field, c.op) };
                  setEditing({ ...editing, conditions });
                }}
              >
                {RULE_FIELDS.map((f) => (
                  <option key={f} value={f}>{t(FIELD_LABEL[f])}</option>
                ))}
              </select>
              {c.field === 'header' && (
                <input
                  className="text-input rule-header-name"
                  placeholder={t('rule-header-placeholder')}
                  aria-label={t('rule-header-placeholder')}
                  value={c.header ?? ''}
                  onChange={(e) => {
                    const conditions = [...editing.conditions];
                    conditions[i] = { ...c, header: e.target.value };
                    setEditing({ ...editing, conditions });
                  }}
                  onKeyDown={(e) => e.stopPropagation()}
                />
              )}
              <select
                className="select"
                value={c.op}
                aria-label={t('rule-op')}
                onChange={(e) => {
                  const conditions = [...editing.conditions];
                  conditions[i] = { ...c, op: e.target.value as typeof c.op };
                  setEditing({ ...editing, conditions });
                }}
              >
                {opsFor(c.field).map((o) => (
                  <option key={o} value={o}>{t(OP_LABEL[o])}</option>
                ))}
              </select>
              <input
                className="text-input"
                type={valueKind(c.field) === 'text' ? 'text' : valueKind(c.field)}
                placeholder={
                  valueKind(c.field) === 'number'
                    ? t('rule-value-kb-placeholder')
                    : t('rule-value-placeholder')
                }
                aria-label={t('rule-value-placeholder')}
                value={c.value}
                onChange={(e) => {
                  const conditions = [...editing.conditions];
                  conditions[i] = { ...c, value: e.target.value };
                  setEditing({ ...editing, conditions });
                }}
                onKeyDown={(e) => e.stopPropagation()}
              />
              {editing.conditions.length > 1 && (
                <button
                  type="button" className="act-icon" aria-label={t('rule-cond-remove')}
                  onClick={() =>
                    setEditing({
                      ...editing,
                      conditions: editing.conditions.filter((_, j) => j !== i),
                    })
                  }
                >
                  <Icon icon={Trash2} size={13} />
                </button>
              )}
            </div>
          ))}
          <button
            type="button" className="fbtn"
            onClick={() =>
              setEditing({
                ...editing,
                conditions: [...editing.conditions, { field: 'subject', op: 'contains', value: '' }],
              })
            }
          >
            <Icon icon={Plus} size={13} />
            {t('rule-cond-add')}
          </button>

          <div className="sublabel">{t('rule-then')}</div>
          <div className="rule-acts">
            {/* Destinations, not the raw folder list. What a rule may file
                into is the same question the move picker answers, and it was
                being answered differently here — every role mailbox, every
                `[Gmail]/…` path and every folder sitting in the bin was on
                offer, and a rule filing mail into a deleted folder is a rule
                that loses it. */}
            <div className="rule-field">
              <span>{t('rule-act-move')}</span>
              <PickerField
                mode="folder"
                label={t('rule-act-move')}
                value={editing.actions.move_to}
                options={filableFolderRows(folders).map((r) => ({
                  id: r.id,
                  label: r.path,
                  depth: r.depth,
                  container: r.container || undefined,
                  hasChildren: r.hasChildren || undefined,
                  anchor: r.anchor,
                }))}
                noneLabel={t('rule-act-move-none')}
                onChange={(id) =>
                  setEditing({ ...editing, actions: { ...editing.actions, move_to: id } })
                }
                onCreate={(name) => {
                  void api
                    .createFolder(name)
                    .then((id) => {
                      setEditing((cur) =>
                        cur ? { ...cur, actions: { ...cur.actions, move_to: id } } : cur,
                      );
                      return api.folders().then(setFolders);
                    })
                    .catch((e) => onMessage(String(e)));
                }}
              />
            </div>
            <div className="rule-field">
              <span>{t('rule-act-tag')}</span>
              <PickerField
                mode="tag"
                label={t('rule-act-tag')}
                value={editing.actions.tag}
                options={tags.map((tg) => ({
                  id: tg.id,
                  label: tg.name,
                  colour: tg.colour || undefined,
                }))}
                noneLabel={t('rule-act-tag-none')}
                onChange={(id) =>
                  setEditing({ ...editing, actions: { ...editing.actions, tag: id } })
                }
                onCreate={(name) => {
                  void api
                    .createTag(name)
                    .then((id) => {
                      setEditing((cur) =>
                        cur ? { ...cur, actions: { ...cur.actions, tag: id } } : cur,
                      );
                      return api.tags().then(setTags);
                    })
                    .catch((e) => onMessage(String(e)));
                }}
              />
            </div>
            <label className={editing.actions.move_to != null ? 'is-moot' : undefined}>
              <input
                type="checkbox"
                checked={editing.actions.skip_inbox && editing.actions.move_to == null}
                // Moving the mail is already skipping the inbox, so the box
                // is not offered as a second, contradictory instruction.
                disabled={editing.actions.move_to != null}
                onChange={(e) =>
                  setEditing({
                    ...editing,
                    actions: { ...editing.actions, skip_inbox: e.target.checked },
                  })
                }
              />
              {editing.actions.move_to != null ? t('rule-act-skip-moot') : t('rule-act-skip')}
            </label>
            <label>
              <input
                type="checkbox"
                checked={editing.actions.mark_read}
                onChange={(e) =>
                  setEditing({
                    ...editing,
                    actions: { ...editing.actions, mark_read: e.target.checked },
                  })
                }
              />
              {t('rule-act-read')}
            </label>
            <label>
              <input
                type="checkbox"
                checked={editing.actions.notify}
                onChange={(e) =>
                  setEditing({
                    ...editing,
                    actions: { ...editing.actions, notify: e.target.checked },
                  })
                }
              />
              {t('rule-act-notify')}
            </label>
          </div>

          <div className="storage-actions">
            <button type="button" className="fbtn primary" onClick={() => void save(editing)}>
              {t('rule-save')}
            </button>
            <button type="button" className="fbtn" onClick={() => setEditing(null)}>
              {t('cancel')}
            </button>
          </div>
        </section>
      )}
    </div>
  );
}
