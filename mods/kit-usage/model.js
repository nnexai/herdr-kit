export const ORDER = ['codex', 'claude', 'copilot'];
const number = { format(n) { const parts = String(Math.round(n * 100) / 100).split('.'); parts[0] = parts[0].replace(/\B(?=(\d{3})+(?!\d))/g, ','); return parts.join('.'); } };
const weekdays = ['Sun','Mon','Tue','Wed','Thu','Fri','Sat'];
const months = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
const pad = n => String(n).padStart(2,'0');
export function formatTime(ms) { const d = new Date(ms); return `${pad(d.getHours())}:${pad(d.getMinutes())}`; }
export function formatDateTime(ms) { const d = new Date(ms); return `${months[d.getMonth()]} ${d.getDate()}, ${d.getFullYear()} ${formatTime(ms)}`; }
const finite = n => typeof n === 'number' && Number.isFinite(n);
const bounded = (n,max) => n === null || (finite(n) && n >= 0 && n <= max);
const timestamp = n => Number.isSafeInteger(n) && n >= 0;
const nullableTimestamp = n => n === null || timestamp(n);
const clamp = n => Math.max(0, Math.min(1, n));

// Copy only the identity-free protocol fields; never persist unknown helper output.
export function decodeSnapshot(text) {
  if (typeof text !== 'string' || text.length > 1048576) throw new Error('malformed');
  const x = JSON.parse(text);
  if (!x || x.schema_version !== 1 || !timestamp(x.generated_at_ms) || typeof x.collecting !== 'boolean' || !Array.isArray(x.providers) || x.providers.length > 3) throw new Error('malformed');
  const seen = new Set();
  const providers = x.providers.map(p => {
    if (!p || !ORDER.includes(p.provider) || seen.has(p.provider) || !['pending','available','not_signed_in','unsupported','unavailable'].includes(p.state) || (p.error !== null && !['source_missing','not_signed_in','usage_unavailable','unsupported','failed','timeout','malformed','cache_unavailable'].includes(p.error)) || !nullableTimestamp(p.fetched_at_ms) || typeof p.stale !== 'boolean' || !Array.isArray(p.accounts) || p.accounts.length > 8) throw new Error('malformed');
    seen.add(p.provider);
    return { provider:p.provider, state:p.state, error:p.error, fetched_at_ms:p.fetched_at_ms, stale:p.stale, accounts:p.accounts.map(a => {
      if (!a || !timestamp(a.fetched_at_ms) || !Array.isArray(a.limits) || a.limits.length > 24) throw new Error('malformed');
      return { fetched_at_ms:a.fetched_at_ms, limits:a.limits.map((l,index) => {
        const isCredits = p.provider === 'copilot';
        if (!l || !(l.window === null || /^(weekly|monthly|\d{1,4}[smhdw])$/.test(l.window)) || !(l.tier === null || ['opus','sonnet','haiku','fable'].includes(l.tier)) || l.id !== `${p.provider}:${l.window ?? 'none'}:${index}` || l.unit !== (isCredits ? 'credits' : 'percent') || !bounded(l.used_fraction,10) || ![l.used,l.limit,l.remaining].every(n => bounded(n,1e9) && (isCredits || n === null)) || typeof l.unlimited !== 'boolean' || (l.unlimited && (!isCredits || l.used_fraction !== null || l.used !== null || l.limit !== null || l.remaining !== null)) || !['ok','warning','exhausted','unknown'].includes(l.level) || !nullableTimestamp(l.resets_at_ms)) throw new Error('malformed');
        return { id:l.id, window:l.window, tier:l.tier, unit:l.unit, used_fraction:l.used_fraction, used:l.used, limit:l.limit, remaining:l.remaining, unlimited:l.unlimited, level:l.level, resets_at_ms:l.resets_at_ms };
      }) };
    }) };
  });
  return { schema_version:1, generated_at_ms:x.generated_at_ms, collecting:x.collecting, providers };
}
export function usedFraction(l) {
  if (l.unlimited) return null;
  if (finite(l.used_fraction)) return clamp(l.used_fraction);
  if (l.unit === 'credits' && l.limit > 0) {
    if (finite(l.remaining)) return clamp((l.limit-l.remaining)/l.limit);
    if (finite(l.used)) return clamp(l.used/l.limit);
  }
  return null;
}
export function credits(l, detail = false) {
  if (l.unit !== 'credits' || l.unlimited) return null;
  const used = l.used ?? (l.limit !== null && l.remaining !== null ? Math.max(0,l.limit-l.remaining) : null);
  if (used === null) return null;
  const fmt = n => !detail && n >= 10000 ? `${number.format(n/1000)}k` : number.format(n);
  return detail ? `${fmt(used)}${l.limit === null ? '' : ` of ${fmt(l.limit)}`} AI credits used` : `${fmt(used)}${l.limit === null ? '' : ` / ${fmt(l.limit)}`} AIC`;
}
export function value(l) { const n = usedFraction(l); return l.unlimited ? 'unlimited' : n === null ? '—' : `${number.format(n*100)}%`; }
export function label(l) { return [l.tier,l.window].filter(Boolean).map(s => /^\d+[smhdw]$/.test(s) ? s : s.replace(/[-_]/g,' ').replace(/^./, c => c.toUpperCase())).join(' ') || 'Limit'; }
export function staleLimit(l, fetched, now) { return l.resets_at_ms !== null && l.resets_at_ms <= now && l.resets_at_ms > fetched; }
export function pace(l, now, stale = false) {
  if (stale || l.unlimited || l.resets_at_ms === null || l.resets_at_ms <= now) return null;
  const match = (l.window || '').match(/^(\d+)([smhdw])$/);
  const length = l.window === 'weekly' ? 604800000 : l.window === 'monthly' ? 2592000000 : match ? Number(match[1]) * ({s:1000,m:60000,h:3600000,d:86400000,w:604800000})[match[2]] : 0;
  const used = usedFraction(l);
  if (!length || used === null) return null;
  const remaining = l.resets_at_ms-now, expected = clamp(1-remaining/length);
  if (expected < .05) return null;
  const out = used > 0 && used < 1 ? (1-used)*expected*length/used : null;
  return { expected, runsOutMs: expected >= .2 && out !== null && out < remaining ? out : null };
}
export function span(ms) { const m = Math.max(1,Math.ceil(ms/60000)); return m >= 1440 ? `${Math.floor(m/1440)}d${Math.floor(m%1440/60) ? ` ${Math.floor(m%1440/60)}h` : ''}` : m >= 60 ? `${Math.floor(m/60)}h${m%60 ? ` ${m%60}m` : ''}` : `${m}m`; }
export function reset(ms, now) {
  if (ms === null) return null;
  if (ms <= now) return 'Reset time passed · awaiting a newer report';
  const d = new Date(ms), delta = ms-now;
  const time = formatTime(ms);
  const date = delta < 86400000 ? time : delta < 604800000 ? `${weekdays[d.getDay()]} ${time}` : `${months[d.getMonth()]} ${d.getDate()} ${time}`;
  return `resets ${date} · in ${span(delta)}`;
}
export function groups(p, now, offline = false) {
  const map = new Map();
  for (const account of p.accounts) for (const limit of account.limits) {
    const key = JSON.stringify([limit.tier,limit.window]), used = usedFraction(limit), prior = map.get(key);
    if (prior && !(used !== null && (prior.used === null || used > prior.used)) && !(used === prior.used && (limit.resets_at_ms ?? Infinity) < (prior.limit.resets_at_ms ?? Infinity))) continue;
    const stale = offline || p.stale || p.error !== null || staleLimit(limit,account.fetched_at_ms,now);
    map.set(key,{limit,used,stale,pace:pace(limit,now,stale)});
  }
  return [...map.values()];
}
export function views(snapshot, now, offline = false) {
  return ORDER.map(name => { const provider = snapshot?.providers.find(p => p.provider === name) ?? {provider:name,state:'pending',error:null,fetched_at_ms:null,stale:false,accounts:[]}; return {provider, name:name[0].toUpperCase()+name.slice(1), segments:groups(provider,now,offline)}; });
}
export function bestSegment(segments, freshOnly = false) {
  return segments.filter(s => !freshOnly || !s.stale).reduce((best,s) => !best || (s.used ?? -1) > (best.used ?? -1) ? s : best,null);
}
export function summary(all) {
  const visible = all.filter(v => v.provider.state !== 'not_signed_in');
  const fresh = visible.filter(v => v.segments.some(s => !s.stale && s.used !== null));
  const pool = fresh.length ? fresh : visible;
  return pool.reduce((best,v) => !best || (bestSegment(v.segments,!!fresh.length)?.used ?? -1) > (bestSegment(best.segments,!!fresh.length)?.used ?? -1) ? v : best,null);
}
export function errorText(error) { return ({source_missing:'OMP usage source not found',not_signed_in:'Not signed in to OMP',usage_unavailable:'Usage unavailable',unsupported:'Not reported for this account',failed:'Usage check failed',timeout:'Usage check timed out',malformed:'Invalid usage report',cache_unavailable:'Shared usage cache unavailable'})[error] || 'Usage unavailable'; }
