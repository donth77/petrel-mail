import { useEffect, useState } from 'react';
import { ArrowDown, ArrowUp, Plus, Trash2 } from 'lucide-react';
import { api, type Folder, type Rule, type RuleCondition, type Tag } from '../../lib/api';
import { Icon } from '../Icon';
import { t } from '../../lib/strings';

const FIELDS = ['from', 'to', 'subject', 'list_id'] as const;

/** A rule summarised in one readable line, so the list explains itself. */
function summary(rule: Rule, folders: Folder[], tags: Tag[]): string {
  const conds = rule.conditions
    .map((c) => `${t(`rule-field-${c.field}` as 'rule-field-from')} ~ “${c.contains}”`)
    .join(' + ');
  const acts: string[] = [];
  const a = rule.actions;
  if (a.move_to != null) {
    const f = folders.find((x) => x.id === a.move_to);
    acts.push(t('rule-sum-move', { folder: f ? f.path : '?' }));
  }
  if (a.skip_inbox) acts.push(t('rule-sum-skip'));
  if (a.tag != null) {
    const tg = tags.find((x) => x.id === a.tag);
    acts.push(t('rule-sum-tag', { tag: tg ? tg.name : '?' }));
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
    conditions: [{ field: 'from', contains: '' }],
    actions: { move_to: null, tag: null, mark_read: false, skip_inbox: false, notify: false },
  };

  const save = async (r: Rule) => {
    const conditions = r.conditions.filter((c) => c.contains.trim());
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
              }
            />
            <button type="button" className="rule-name" onClick={() => setEditing(r)}>
              <span>{r.name}</span>
              <span className="rule-sum">{summary(r, folders, tags)}</span>
            </button>
            <button
              type="button" className="act-icon" aria-label={t('rule-up')}
              disabled={i === 0}
              onClick={() => void api.moveRule(r.id, true).then(reload)}
            >
              <Icon icon={ArrowUp} size={13} />
            </button>
            <button
              type="button" className="act-icon" aria-label={t('rule-down')}
              disabled={i === rules.length - 1}
              onClick={() => void api.moveRule(r.id, false).then(reload)}
            >
              <Icon icon={ArrowDown} size={13} />
            </button>
            <button
              type="button" className="act-icon" aria-label={t('rule-delete')}
              onClick={() => void api.deleteRule(r.id).then(reload)}
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
                  const conditions = [...editing.conditions];
                  conditions[i] = { ...c, field: e.target.value as RuleCondition['field'] };
                  setEditing({ ...editing, conditions });
                }}
              >
                {FIELDS.map((f) => (
                  <option key={f} value={f}>{t(`rule-field-${f}` as 'rule-field-from')}</option>
                ))}
              </select>
              <input
                className="text-input"
                placeholder={t('rule-contains-placeholder')}
                aria-label={t('rule-contains-placeholder')}
                value={c.contains}
                onChange={(e) => {
                  const conditions = [...editing.conditions];
                  conditions[i] = { ...c, contains: e.target.value };
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
                conditions: [...editing.conditions, { field: 'subject', contains: '' }],
              })
            }
          >
            <Icon icon={Plus} size={13} />
            {t('rule-cond-add')}
          </button>

          <div className="sublabel">{t('rule-then')}</div>
          <div className="rule-acts">
            <label>
              {t('rule-act-move')}
              <select
                className="select"
                value={editing.actions.move_to ?? ''}
                onChange={(e) =>
                  setEditing({
                    ...editing,
                    actions: {
                      ...editing.actions,
                      move_to: e.target.value ? Number(e.target.value) : null,
                    },
                  })
                }
              >
                <option value="">{t('rule-act-move-none')}</option>
                {folders.map((f) => (
                  <option key={f.id} value={f.id}>{f.path}</option>
                ))}
              </select>
            </label>
            <label>
              {t('rule-act-tag')}
              <select
                className="select"
                value={editing.actions.tag ?? ''}
                onChange={(e) =>
                  setEditing({
                    ...editing,
                    actions: {
                      ...editing.actions,
                      tag: e.target.value ? Number(e.target.value) : null,
                    },
                  })
                }
              >
                <option value="">{t('rule-act-tag-none')}</option>
                {tags.map((tg) => (
                  <option key={tg.id} value={tg.id}>{tg.name}</option>
                ))}
              </select>
            </label>
            <label>
              <input
                type="checkbox"
                checked={editing.actions.skip_inbox}
                onChange={(e) =>
                  setEditing({
                    ...editing,
                    actions: { ...editing.actions, skip_inbox: e.target.checked },
                  })
                }
              />
              {t('rule-act-skip')}
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
