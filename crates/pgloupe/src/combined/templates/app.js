/* The dashboard. It reads the report embedded in the page and renders it with textContent only. */
(function () {
  'use strict';
  var L = PgLib, C = PgCharts;
  var dataNode = document.getElementById('pgloupe-data');
  var root = document.getElementById('app');
  if (!dataNode || !root) { return; }
  var state = L.parseHash(location.hash);
  var rawJson, D, logs, stats, rows, findings, advice, actions, errorsByFp, byFp, newShare, hideNew, display, sections;

  /** Reads a report (JSON text) into the derived state; false if it is not one. */
  function load(raw) {
    var data;
    try { data = JSON.parse(raw); } catch (e) { return false; }
    if (!data || data.kind !== 'pgloupe.report') { return false; }
    rawJson = raw;
    D = data;
    L.setZone(D.display_tz);
    logs = D.logs || null;
    stats = D.inputs && D.inputs.stats;
    rows = D.statements || [];
    findings = D.findings || [];
    advice = D.advice || [];
    actions = D.actions || [];
    errorsByFp = Object.create(null);
    if (logs) { (logs.errors || []).forEach(function (e) { (e.statements || []).forEach(function (s) { (errorsByFp[s.fingerprint] = errorsByFp[s.fingerprint] || []).push({ error: e, count: s.count }); }); }); }
    var withStats = rows.filter(function (r) { return r.stats; });
    var fresh = withStats.filter(function (r) { return r.stats.flags.indexOf('new') !== -1; });
    newShare = withStats.length ? fresh.length / withStats.length : 0;
    hideNew = newShare > 0.8;
    if (hideNew && state.flag === 'new') { state.flag = ''; }
    byFp = Object.create(null);
    rows.forEach(function (r) { byFp[r.fingerprint] = r; });
    display = logs ? L.downsample(logs.timeline.buckets, 120) : [];
    sections = [
      { id: 'overview', label: 'Overview', show: true },
      { id: 'statements', label: 'Statements', show: rows.length > 0 },
      { id: 'errors', label: 'Errors', show: !!logs },
      { id: 'locks', label: 'Locks & temp files', show: !!logs },
      { id: 'sessions', label: 'Sessions', show: !!logs },
      { id: 'maintenance', label: 'Maintenance', show: !!logs },
      { id: 'breakdowns', label: 'Breakdowns', show: !!logs || !!(stats && stats.by_database && stats.by_database.length) },
      { id: 'slowest', label: 'Slowest', show: !!(logs && logs.slowest && logs.slowest.rows.length) },
      { id: 'pooler', label: 'Pooler', show: !!(logs && logs.pooler) },
      { id: 'comparison', label: 'Comparison', show: !!D.comparison },
      { id: 'advice', label: 'Settings', show: advice.length > 0 },
      { id: 'sources', label: 'Sources', show: true },
      { id: 'findings', label: 'Data caveats', show: findings.length > 0 }
    ].filter(function (s) { return s.show; });
    if (!sections.some(function (s) { return s.id === state.s; })) { state.s = 'overview'; }
    if (state.fp && !byFp[state.fp]) { state.fp = ''; }
    return true;
  }
  if (!load(dataNode.textContent)) { return; }
  function visibleFlags(r) { return r.stats ? r.stats.flags.filter(function (f) { return !(hideNew && f === 'new'); }) : []; }

  /** Rows of the activity chart. Rates draw as areas, events as ticks; rows with nothing in them are left out. */
  var SERIES = [
    { key: 'statements', label: 'Statements', color: 'var(--c-statements)', kind: 'rate', visible: true },
    { key: 'connections', label: 'Connections', color: 'var(--c-connections)', kind: 'rate', visible: true },
    { key: 'lock_waits', label: 'Lock waits', color: 'var(--c-locks)', kind: 'rate', visible: true },
    { key: 'wal_kb', label: 'WAL written', color: '#8b5cf6', kind: 'rate', visible: true, fmt: function (kb) { return L.fmtBytes(kb * 1024); } },
    { key: 'errors', label: 'Errors', color: 'var(--c-errors)', kind: 'event', visible: true },
    { key: 'aborts', label: 'Abrupt endings', color: '#d97706', kind: 'event', visible: true },
    { key: 'checkpoints', label: 'Checkpoints', color: '#64748b', kind: 'event', visible: true },
    { key: 'ddl', label: 'Schema changes', color: '#d946ef', kind: 'event', visible: true }
  ];
  var KIND_COLORS = { select: '#4f7cff', insert: '#12a594', update: '#f0a020', delete: '#e5484d', copy: '#8b5cf6', ddl: '#d946ef', tcl: '#64748b', utility: '#94a3b8', other: '#cbd5e1' };


  /* ---------- helpers ---------- */
  function h(tag, attrs, kids) {
    var node = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (key) {
      var v = attrs[key];
      if (v === null || v === undefined || v === false) { return; }
      if (key === 'class') { node.className = v; }
      else if (key === 'text') { node.textContent = v; }
      else if (key.slice(0, 2) === 'on') { node.addEventListener(key.slice(2), v); }
      else if (key === 'style') { Object.keys(v).forEach(function (k) { node.style.setProperty(k, v[k]); }); }
      else if (v === true) { node.setAttribute(key, ''); }
      else { node.setAttribute(key, String(v)); }
    });
    append(node, kids);
    return node;
  }
  function append(node, kids) {
    if (kids === null || kids === undefined || kids === false) { return; }
    if (Array.isArray(kids)) { kids.forEach(function (k) { append(node, k); }); }
    else if (typeof kids === 'string' || typeof kids === 'number') { node.appendChild(document.createTextNode(String(kids))); }
    else { node.appendChild(kids); }
  }
  function store(key, value) { try { if (value === undefined) { return localStorage.getItem(key); } localStorage.setItem(key, value); } catch (e) { return null; } return null; }
  function toast(message) {
    var t = h('div', { 'class': 'toast', role: 'status', text: message });
    document.body.appendChild(t);
    setTimeout(function () { t.remove(); }, 1600);
  }
  function copy(textValue, what) {
    var done = function () { toast(what + ' copied'); };
    if (navigator.clipboard && navigator.clipboard.writeText) { navigator.clipboard.writeText(textValue).then(done, function () { toast('Copy failed'); }); }
    else { toast('Copy is not available here'); }
  }
  function badge(textValue, cls) { return h('span', { 'class': 'badge ' + (cls || ''), text: textValue }); }
  function num(textValue) { return textValue; }
  function sqlNode(sql, cls) {
    var pre = h('pre', { 'class': 'sql-block ' + (cls || '') });
    L.tokenizeSql(sql).forEach(function (tok) {
      if (tok.t === 'ws' || tok.t === 'op' || tok.t === 'id') { pre.appendChild(document.createTextNode(tok.v)); }
      else { pre.appendChild(h('span', { 'class': tok.t, text: tok.v })); }
    });
    return pre;
  }
  function panel(title, body, extra) {
    return h('section', { 'class': 'panel' }, [title ? h('h3', null, [title, extra ? h('span', { 'class': 'grow' }) : null, extra || null]) : null, body]);
  }
  function kpi(label, value, hint, spark, tone) {
    return h('div', { 'class': 'panel kpi ' + (tone || '') }, [
      h('span', { 'class': 'label', text: label }),
      h('span', { 'class': 'value', text: value }),
      h('span', { 'class': 'hint', text: hint || '' }),
      spark || null
    ]);
  }
  /** A sparkline only when the series moves: a flat 0-or-1 line says nothing. */
  function sparkIf(key, color) { var v = series(key); return Math.max.apply(null, v.concat([0])) > 2 ? C.sparkline(v, color) : null; }
  function series(key) { return display.map(function (b) { return b[key] || 0; }); }
  function emptyNote(message) { return h('div', { 'class': 'empty', text: message }); }
  function openStatement(fp) { setState({ fp: fp }); }

  /* ---------- state ---------- */
  var drawerCursor = null;
  function setState(patch, opts) {
    Object.keys(patch).forEach(function (k) { state[k] = patch[k]; });
    var hash = L.buildHash(state);
    try { history.replaceState(null, '', hash || location.pathname + location.search); } catch (e) { location.hash = hash; }
    if (patch.s !== undefined || (opts && opts.render)) { render(); }
    else if (patch.fp !== undefined) { drawer(); markSelected(); }
    else if (patch.from !== undefined || patch.to !== undefined) { render(); }
  }
  function theme() { return state.th || store('pgloupe.theme') || ''; }
  function applyTheme() {
    var t = theme();
    if (t === 'light' || t === 'dark') { document.documentElement.setAttribute('data-theme', t); }
    else { document.documentElement.removeAttribute('data-theme'); }
  }
  function cycleTheme() {
    var next = { '': 'light', light: 'dark', dark: '' }[theme()] || '';
    store('pgloupe.theme', next);
    state.th = '';
    applyTheme();
    renderChrome();
    toast('Theme: ' + (next || 'automatic'));
  }

  /* ---------- sections ---------- */
  var chrome = {};
  function renderChrome() {
    var warn = findings.filter(function (f) { return f.severity === 'warn'; }).length;
    var nav = h('nav', { 'aria-label': 'Sections' }, sections.map(function (s, i) {
      return h('button', {
        'class': 'nav-item', 'aria-current': s.id === state.s ? 'page' : null,
        onclick: function () { setState({ s: s.id, fp: '' }); window.scrollTo(0, 0); }
      }, [s.label, s.id === 'findings' ? badge(String(findings.length), warn ? 'warn' : 'info') : null, s.id === 'overview' && actions.filter(function (a) { return a.priority !== 'low'; }).length ? badge(String(actions.filter(function (a) { return a.priority !== 'low'; }).length), actions.some(function (a) { return a.priority === 'high'; }) ? 'bad' : 'warn') : null]);
    }));
    var logo = h('span', { 'class': 'brand' }, [
      C.svgEl('svg', { viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': '2', 'stroke-linecap': 'round' }, [
        C.svgEl('circle', { cx: 10, cy: 10, r: 6.5 }),
        C.svgEl('line', { x1: 15, y1: 15, x2: 21, y2: 21 })
      ]),
      'pgloupe'
    ]);
    var meta = h('div', { 'class': 'meta' }, [D.generator, h('br'), 'schema ' + D.schema_version, h('br'), 'keys: / search, j k, 1-' + sections.length]);
    var rail = h('aside', { 'class': 'rail' }, [logo, nav, h('div', { 'class': 'spacer' }), meta]);
    var parts = ['times in ' + (D.display_tz || 'UTC')];
    if (D.windows && D.windows.logs) { parts.push('logs ' + L.fmtTime(D.windows.logs.from, true) + ' \u2192 ' + L.fmtTime(D.windows.logs.to, true)); }
    if (D.windows && D.windows.stats) { parts.push('statistics ' + L.fmtTime(D.windows.stats.from, true) + ' \u2192 ' + L.fmtTime(D.windows.stats.to, true)); }
    if (D.windows && D.windows.overlap_pct !== null && D.windows.overlap_pct !== undefined) { parts.push('overlap ' + Math.round(D.windows.overlap_pct) + '%'); }
    var subs = [h('span', { text: parts.join(' \u00b7 ') })];
    var filtered = logs && logs.diagnostics ? logs.diagnostics.filtered : 0;
    if (filtered) { subs.push(badge('filtered: ' + L.fmtInt(filtered) + ' records dropped', 'warn')); }
    var themeName = theme() || 'auto';
    var top = h('div', { 'class': 'topbar' }, [
      h('div', null, [h('h1', { text: (sections.filter(function (s) { return s.id === state.s; })[0] || sections[0]).label }), h('div', { 'class': 'sub' }, subs)]),
      h('div', { 'class': 'actions' }, [
        h('button', { 'class': 'btn', title: 'Switch theme', onclick: cycleTheme, text: 'Theme: ' + themeName }),
        h('button', { 'class': 'btn', title: 'Download the report data', onclick: downloadJson, text: 'JSON' }),
        h('button', { 'class': 'btn', title: 'Print the static report', onclick: function () { window.print(); }, text: 'Print' })
      ])
    ]);
    chrome.rail = rail; chrome.top = top;
    var old = root.querySelector('.rail');
    if (old) { root.replaceChild(rail, old); } else { root.insertBefore(rail, root.firstChild); }
    var mainNode = root.querySelector('.main');
    if (!mainNode) { mainNode = h('div', { 'class': 'main', role: 'main' }); root.appendChild(mainNode); }
    var oldTop = mainNode.querySelector('.topbar');
    if (oldTop) { mainNode.replaceChild(top, oldTop); } else { mainNode.insertBefore(top, mainNode.firstChild); }
  }
  function downloadJson() {
    var blob = new Blob([rawJson], { type: 'application/json' });
    var a = h('a', { href: URL.createObjectURL(blob), download: 'pgloupe-report.json' });
    document.body.appendChild(a); a.click(); a.remove();
    setTimeout(function () { URL.revokeObjectURL(a.href); }, 1000);
  }

  /* ---------- overview ---------- */
  function rangeNow() { return L.clampRange(state.from, state.to, display.length); }
  /** True when the log's own diagnostics say this kind of line never appeared. */
  function notLogged(word) {
    var warnings = (logs && logs.diagnostics && logs.diagnostics.warnings) || [];
    return warnings.some(function (w) { return w.indexOf('not seen:') === 0 && w.indexOf(word) !== -1; });
  }
  /** A KPI whose source is absent says so, instead of showing a zero that looks like good news. */
  function offKpi(label, why) { return kpi(label, 'not logged', why, null, 'off'); }
  var DASH = '—';
  function pctText(v) { return v === null || v === undefined ? DASH : L.fmtPct(v, 1); }

  function healthKpis(sum, scope) {
    var db = D.database && D.database.total;
    var out = [];
    if (logs) {
      var errors = sum('errors');
      out.push(kpi('Errors', L.fmtCompact(errors), scope, sparkIf('errors', 'var(--c-errors)'), errors > 0 ? 'bad' : ''));
      var cancelled = logs.cancelled ? logs.cancelled.total : 0;
      out.push(kpi('Cancelled', L.fmtInt(cancelled), cancelled ? reasonsText(logs.cancelled) : 'none', null, cancelled ? 'warn' : ''));
    } else {
      out.push(offKpi('Errors', 'no log was given'));
      out.push(offKpi('Cancelled', 'no log was given'));
    }
    if (logs && !(notLogged('lock waits') && !logs.locks.waits)) {
      out.push(kpi('Lock waits', L.fmtCompact(sum('lock_waits')), 'total wait ' + L.fmtMs(logs.locks.total_wait_ms) + ', ' + logs.locks.deadlocks + ' deadlocks', sparkIf('lock_waits', 'var(--c-locks)'), logs.locks.deadlocks ? 'warn' : ''));
    } else if (db) {
      out.push(kpi('Deadlocks', L.fmtInt(db.deadlocks), 'from pg_stat_database; lock waits are not logged', null, db.deadlocks ? 'warn' : ''));
    } else {
      out.push(offKpi('Lock waits', 'log_lock_waits is off, or none happened'));
    }
    if (logs && !(notLogged('temp files') && !logs.temp_files.files)) {
      out.push(kpi('Temp files', L.fmtBytes(logs.temp_files.bytes), L.fmtInt(logs.temp_files.files) + ' files, largest ' + L.fmtBytes(logs.temp_files.max_bytes)));
    } else if (db) {
      out.push(kpi('Temp files', L.fmtBytes(db.temp_bytes), L.fmtInt(db.temp_files) + ' files, from pg_stat_database'));
    } else {
      out.push(offKpi('Temp files', 'log_temp_files is off, or none happened'));
    }
    if (db) {
      out.push(kpi('Rollbacks', pctText(db.rollback_pct), L.fmtInt(db.rollbacks) + ' of ' + L.fmtInt(db.commits + db.rollbacks) + ' transactions', null, db.rollback_pct >= 5 && db.rollbacks >= 100 ? 'warn' : ''));
      out.push(kpi('Abandoned sessions', pctText(db.abandoned_pct), L.fmtInt(db.sessions_abandoned) + ' of ' + L.fmtInt(db.sessions) + ' ended without a clean goodbye', null, db.abandoned_pct >= 20 && db.sessions_abandoned >= 50 ? 'warn' : ''));
    } else {
      out.push(offKpi('Rollbacks', 'needs pg_stat_database (snap writes it)'));
      var abrupt = logs && logs.disconnects ? (logs.disconnects.reasons || []).reduce(function (n, r) { return n + r.count; }, 0) : 0;
      out.push(logs && abrupt ? kpi('Abrupt endings', L.fmtCompact(abrupt), 'sessions that ended without a clean goodbye', null, 'warn') : offKpi('Abandoned sessions', 'needs pg_stat_database or connection logging'));
    }
    return out;
  }
  function loadKpis() {
    var db = D.database;
    var totals = stats && stats.totals;
    if (!db && !totals) { return null; }
    var out = [];
    if (db) {
      var t = db.total;
      out.push(kpi('Running statements', t.avg_active.toFixed(1), 'average sessions over ' + L.fmtInt(db.window_secs) + ' s'));
      out.push(kpi('Idle in transaction', t.avg_idle_in_transaction.toFixed(1), 'average sessions', null, t.avg_idle_in_transaction >= 1 ? 'warn' : ''));
      out.push(kpi('Connected', t.avg_sessions.toFixed(1), 'average sessions, ' + L.fmtInt(t.backends_now) + ' at the end'));
      out.push(kpi('Cache hit', pctText(t.hit_pct), L.fmtInt(t.blocks_read) + ' blocks read'));
    } else if (totals) {
      out.push(kpi('Cache hit', pctText(totals.hit_pct), L.fmtInt(totals.blocks_read) + ' blocks read'));
    }
    if (totals) {
      out.push(totals.io_pct > 0
        ? kpi('File I/O', L.fmtPct(totals.io_pct, 0), L.fmtMs(totals.io_ms) + ' of ' + L.fmtMs(totals.exec_ms) + ' executing', null, totals.io_pct >= 30 ? 'warn' : '')
        : kpi('File I/O', 'not tracked', 'track_io_timing is off, or nothing waited on a file', null, 'off'));
      if (totals.plan_pct) { out.push(kpi('Planning', L.fmtPct(totals.plan_pct, 0), L.fmtMs(totals.plan_ms) + ' of execution time')); }
    }
    if (logs && logs.latency && logs.latency.count) {
      out.push(kpi('Statement p95', L.fmtMs(logs.latency.p95), 'median ' + L.fmtMs(logs.latency.p50) + ', slowest ' + L.fmtMs(logs.latency.max)));
    }
    if (db && db.statements_coverage_pct !== null && db.statements_coverage_pct !== undefined) {
      var c = db.statements_coverage_pct;
      out.push(kpi('pg_stat_statements covers', c < 1 ? 'under 1%' : Math.round(c) + '%', 'of the time the server spent running statements', null, c < 50 ? 'warn' : ''));
    }
    return out;
  }
  function actionRow(a, index) {
    var links = (a.statements || []).map(function (fp) {
      if (!byFp[fp]) { return h('span', { 'class': 'chip-off', title: 'Not among the statements shown (raise --top)', text: L.shortFp(fp) }); }
      return h('button', { title: 'Open the statement', onclick: function () { openStatement(fp); }, text: L.shortFp(fp) });
    });
    return h('details', { 'class': 'action-row ' + a.priority }, [
      h('summary', null, [h('span', { 'class': 'num', text: String(index + 1) }), h('span', { 'class': 'title', text: a.title }), badge(a.priority, a.priority === 'high' ? 'bad' : a.priority === 'low' ? 'info' : 'warn')]),
      h('div', { 'class': 'body' }, [
        h('div', { 'class': 'advice', text: a.advice }),
        a.evidence ? h('code', { 'class': 'evidence', text: (a.evidence_kind === 'statement' ? 'statement: ' : 'log: ') + a.evidence }) : null,
        h('div', { 'class': 'links' }, [
          links.length ? h('span', { 'class': 'muted', text: 'statements' }) : null, links,
          h('button', { 'class': 'btn small', onclick: function () { setState({ s: SECTION_FOR[a.section] || 'overview', fp: '' }); window.scrollTo(0, 0); }, text: 'Details' })
        ])
      ])
    ]);
  }
  function attentionPanel() {
    var body = [];
    if (actions.length) { body.push(h('div', { 'class': 'todo' }, actions.map(actionRow))); }
    if (findings.length) {
      var warns = findings.filter(function (f) { return f.severity === 'warn'; }).length;
      body.push(h('div', { 'class': 'caveat-line' }, [
        badge(String(findings.length), warns ? 'warn' : 'info'),
        h('span', { text: ' data caveats' + (warns ? ' (' + warns + ' warnings)' : '') + ': ' + findings[0].code.replace(/_/g, ' ') + (findings.length > 1 ? ' and ' + (findings.length - 1) + ' more' : '') + ' ' }),
        h('button', { 'class': 'btn small', onclick: function () { setState({ s: 'findings' }); window.scrollTo(0, 0); }, text: 'Read them' })
      ]));
    }
    if (!body.length) { return null; }
    return panel(actions.length ? 'What to fix first' : 'Data caveats', h('div', null, body));
  }
  function topStatementsPanel() {
    if (!rows.length) { return null; }
    var top = L.sortRows(rows, function (r) { return L.column(r, 'total'); }, 'desc').slice(0, 5);
    var heads = [{ label: 'Statement' }, { label: 'Calls', num: true }, { label: 'Total', num: true }, { label: 'Mean', num: true }, { label: 'Share', num: true }]
      .concat(stats ? [{ label: 'Cache hit', num: true }] : []);
    var body = top.map(function (r) {
      var cells = [
        h('td', { 'class': 'stmt-cell' }, h('code', { text: r.text.replace(/\s+/g, ' ').slice(0, 110) })),
        h('td', { 'class': 'num', text: L.fmtInt(L.column(r, 'calls')) }),
        h('td', { 'class': 'num', text: L.fmtMs(L.column(r, 'total')) }),
        h('td', { 'class': 'num', text: L.fmtMs(L.column(r, 'mean')) }),
        h('td', { 'class': 'num', text: L.fmtPct(L.column(r, 'share')) })
      ];
      if (stats) { cells.push(h('td', { 'class': 'num', text: pctText(L.column(r, 'hit')) })); }
      return h('tr', { 'class': 'clickable', tabindex: '0', onclick: function () { openStatement(r.fingerprint); } }, cells);
    });
    return panel('Where the time went', h('div', null, [simpleTable(heads, body)]),
      h('button', { 'class': 'btn small', onclick: function () { setState({ s: 'statements', sort: 'total', dir: 'desc' }); window.scrollTo(0, 0); }, text: 'All statements' }));
  }
  function kindsTable() {
    var kinds = logs && logs.kinds || [];
    if (!kinds.length) { return null; }
    var heads = [{ label: 'Kind' }, { label: 'Calls', num: true }, { label: 'Time', num: true }, { label: 'Mean', num: true }];
    var body = kinds.map(function (k) {
      return h('tr', { 'class': 'clickable', tabindex: '0', title: 'Show only these statements', onclick: function () { setState({ s: 'statements', kind: k.kind, fp: '' }); window.scrollTo(0, 0); } }, [
        h('td', null, h('span', { 'class': 'badge kind ' + k.kind, text: k.kind })),
        h('td', { 'class': 'num', text: L.fmtInt(k.calls) }),
        h('td', { 'class': 'num', text: L.fmtPct(k.share_pct, 0) }),
        h('td', { 'class': 'num', text: L.fmtMs(k.mean_ms) })
      ]);
    });
    return panel('What the statements do', simpleTable(heads, body));
  }
  function viewOverview() {
    var parts = [];
    var range = rangeNow();
    var sum = function (key) { return L.sumRange(display, key, range); };
    var scope = range ? 'in selection' : 'whole period';
    parts.push(h('div', { 'class': 'grid kpis' }, healthKpis(sum, scope)));
    var load = loadKpis();
    if (load) { parts.push(h('div', { 'class': 'grid kpis' }, load)); }
    var attention = attentionPanel();
    if (attention) { parts.push(attention); }
    if (logs) { parts.push(timelinePanel()); }
    var top = topStatementsPanel();
    var kinds = kindsTable();
    if (top && kinds) { parts.push(h('div', { 'class': 'grid cols-2-1' }, [top, kinds])); } else if (top || kinds) { parts.push(top || kinds); }
    if (logs && logs.histograms && logs.histograms.statements.some(function (b) { return b.count; })) {
      parts.push(histogramPanel('Statement durations', logs.histograms.statements, 'var(--c-statements)', logs.overview.records,
        logs.latency && logs.latency.count ? 'median ' + L.fmtMs(logs.latency.p50) + ' \u00b7 p95 ' + L.fmtMs(logs.latency.p95) + ' \u00b7 slowest ' + L.fmtMs(logs.latency.max) + ' \u00b7 ' + L.fmtInt(logs.latency.count) + ' statements timed' : null));
    } else if (logs) {
      parts.push(panel('Statement durations', emptyNote('No statement durations in this log: log_min_duration_statement is off, or nothing was slow enough.')));
    }
    if (D.database) { parts.push(databasePanel(D.database)); }
    if (!logs && !stats) { parts.push(emptyNote('Nothing to show.')); }
    return parts;
  }
  /** What was read and how: the files, the statistics window, the settings behind the advice. */
  function viewSources() {
    var out = [];
    if (logs) {
      out.push(diagnosticsPanel());
    } else {
      out.push(panel('Logs', emptyNote('No log was given.')));
    }
    if (stats) { out.push(statsPanel()); } else { out.push(panel('pg_stat_statements', emptyNote('No statistics snapshots were given.'))); }
    if (D.database) {
      out.push(panel('pg_stat_database', h('dl', { 'class': 'facts' }, [['From', L.fmtTime(D.database.before_at, true)], ['To', L.fmtTime(D.database.after_at, true)], ['Window', L.fmtDuration(D.database.window_secs)], ['Partial', D.database.total.partial ? 'yes (a reset, or a new database)' : 'no']].map(function (f) { return h('div', null, [h('dt', { text: f[0] }), h('dd', { text: String(f[1]) })]); }))));
    } else if (stats) {
      out.push(panel('pg_stat_database', emptyNote('Not given: snap writes pg_stat_database_<time>.csv beside each snapshot, and report finds it there (or use --before-db and --after-db).')));
    }
    return out;
  }
  var SECTION_FOR = { errors: 'errors', locks: 'locks', statements: 'statements' };
  function actionNode(a, index) {
    var links = (a.statements || []).map(function (fp) {
      if (!byFp[fp]) { return h('span', { 'class': 'chip-off', title: 'Not among the statements shown (raise --top)', text: L.shortFp(fp) }); }
      return h('button', { title: 'Open the statement', onclick: function () { openStatement(fp); }, text: L.shortFp(fp) });
    });
    return h('div', { 'class': 'action ' + a.priority }, [
      h('span', { 'class': 'num', text: String(index + 1) }),
      h('div', { 'class': 'body' }, [
        h('div', { 'class': 'title' }, [a.title, ' ', badge(a.priority, a.priority === 'high' ? 'bad' : a.priority === 'low' ? 'info' : 'warn')]),
        h('div', { 'class': 'advice', text: a.advice }),
        a.evidence ? h('code', { 'class': 'evidence', text: (a.evidence_kind === 'statement' ? 'statement: ' : 'log: ') + a.evidence }) : null,
        h('div', { 'class': 'links' }, [
          links.length ? h('span', { 'class': 'muted', text: 'statements' }) : null, links,
          h('button', { 'class': 'btn small', onclick: function () { setState({ s: SECTION_FOR[a.section] || 'overview', fp: '' }); window.scrollTo(0, 0); }, text: 'Details' })
        ])
      ])
    ]);
  }
  function actionsPanel() {
    return panel('What to fix first', h('div', { 'class': 'todo' }, actions.map(actionNode)));
  }
  function diagnosticsPanel() {
    var d = logs.diagnostics;
    var body = [facts([['Lines', L.fmtInt(d.lines)], ['Records', L.fmtInt(d.records)], ['Skipped', L.fmtInt(d.skipped)], ['Unparsed statements', L.fmtInt(d.unparsed_statements)], ['Filtered out', L.fmtInt(d.filtered)]])];
    if (d.filtered) { body.push(h('div', { 'class': 'findings' }, h('div', { 'class': 'finding warn' }, [h('span', { 'class': 'dot' }), h('div', { text: 'Filtered out ' + L.fmtInt(d.filtered) + ' records: this report covers only what the filters kept.' })]))); }
    (d.warnings || []).forEach(function (w) { body.push(h('p', { 'class': 'muted', text: w })); });
    if ((d.unmatched_shapes || []).length) {
      body.push(h('p', { 'class': 'muted', text: 'Most common lines that could not be decoded:' }));
      d.unmatched_shapes.forEach(function (sh) { body.push(h('div', null, [L.fmtInt(sh.count) + '\u00d7 ', h('code', { text: sh.shape }), ' (first at line ' + sh.first_line + ')'])); });
    }
    body.push(h('p', { 'class': 'muted', text: 'Files: ' + (logs.files || []).map(function (f) { return f.path + ' (' + f.format + ', ' + L.fmtBytes(f.bytes) + ')'; }).join('; ') }));
    return panel('Diagnostics', h('div', { 'class': 'grid' }, body));
  }
  function reasonsText(c) { return c.by_reason.map(function (r) { return r.reason.replace('_', ' ') + ' ' + r.count; }).join(', '); }
  function statsPanel() {
    var c = stats.counts || {};
    var facts = [
      ['Server', 'PostgreSQL ' + Math.floor(stats.server_version_num / 10000)],
      ['Counters reset', stats.reset],
      ['Evictions', stats.evictions === null || stats.evictions === undefined ? 'unknown' : L.fmtInt(stats.evictions) + ' rounds' + (stats.evicted_estimate ? ' (about ' + L.fmtInt(stats.evicted_estimate) + ' entries)' : '')],
      ['Entries', L.fmtInt(c.entries)], ['New', L.fmtInt(c['new'])], ['Gone', L.fmtInt(c.gone)], ['Restarted', L.fmtInt(c.restarted)]
    ];
    var tot = stats.totals || {};
    if (tot.plan_pct) { facts.push(['Planning time', L.fmtMs(tot.plan_ms) + ' (' + L.fmtPct(tot.plan_pct, 0) + ' of execution)']); }
    if (tot.nested_entries) { facts.push(['Statements inside functions', L.fmtInt(tot.nested_entries) + ' entries (pg_stat_statements.track = all)']); }
    return panel('pg_stat_statements window', h('dl', { 'class': 'facts' }, facts.map(function (f) { return h('div', null, [h('dt', { text: f[0] }), h('dd', { text: String(f[1]) })]); })));
  }
  function databasePanel(db) {
    var pct = function (v) { return v === null || v === undefined ? '-' : L.fmtPct(v, 1); };
    var heads = ['Database', 'Running', 'Idle in txn', 'Rollbacks', 'Cache hit', 'Abandoned', 'Temp files', 'Deadlocks'].map(function (label, i) { return { label: label, num: i > 0 }; });
    var table = simpleTable(heads, db.databases.map(function (d) {
      var cells = [d.name, d.avg_active.toFixed(2), d.avg_idle_in_transaction.toFixed(2), pct(d.rollback_pct), pct(d.hit_pct), pct(d.abandoned_pct), L.fmtInt(d.temp_files), L.fmtInt(d.deadlocks)];
      return h('tr', null, cells.map(function (c, i) { return h('td', { 'class': i > 0 ? 'num' : '', text: String(c) }); }));
    }));
    var more = db.more_databases ? h('p', { 'class': 'muted', text: db.more_databases + ' more databases are not listed.' }) : null;
    return panel('Databases (pg_stat_database)', h('div', null, [table, more].filter(Boolean)));
  }
  var redraws = [];
  /** The activity chart restricted to some series, for the views of one topic. */
  function chartPanel(title, keys, note) {
    var host = h('div', { 'class': 'chart-host' });
    var chosen = SERIES.filter(function (s) { return keys.indexOf(s.key) !== -1; }).map(function (s) { return Object.assign({}, s, { visible: true }); });
    var draw = function () { C.timeline(host, { buckets: display, series: chosen, range: null }); };
    redraws.push(draw);
    setTimeout(draw, 0);
    return panel(title, h('div', null, [host, h('p', { 'class': 'muted range-note', text: note || ('Each bar covers ' + L.fmtDuration(barSeconds()) + '.') })]));
  }
  /** Seconds one bar of the activity chart covers: the report's bucket times how many were merged. */
  function barSeconds() { return ((logs && logs.timeline.bucket_secs) || 60) * (display.length ? display[0].span : 1); }
  function timelinePanel() {
    var host = h('div', { 'class': 'chart-host' });
    var live = SERIES.filter(function (s) { return display.some(function (b) { return b[s.key] > 0; }); });
    var legend = h('div', { 'class': 'legend' }, live.map(function (s) {
      return h('button', {
        'aria-pressed': String(s.visible), title: 'Show or hide ' + s.label,
        onclick: function () { s.visible = !s.visible; draw(); legend.querySelectorAll('button').forEach(function (b, i) { b.setAttribute('aria-pressed', String(live[i].visible)); }); }
      }, [h('i', { style: { background: s.color } }), s.label]);
    }));
    var note = h('div', { 'class': 'range-note' });
    function draw() {
      C.timeline(host, {
        buckets: display, series: SERIES, range: rangeNow(),
        onRange: function (r) { setState(r ? { from: r.from, to: r.to } : { from: null, to: null }); }
      });
      C.clear(note);
      var r = rangeNow();
      if (r) {
        note.appendChild(document.createTextNode('Selection ' + L.fmtTime(display[r.from].start, true) + ' → ' + L.fmtTime(display[r.to].start, true) + '. Cards follow the selection; statement rows cover the whole period.'));
        note.appendChild(h('button', { 'class': 'btn small', onclick: function () { setState({ from: null, to: null }); }, text: 'Clear' }));
      } else {
        note.appendChild(document.createTextNode('Drag across the chart to select a period. Each bar covers ' + L.fmtDuration(barSeconds()) + '.'));
      }
    }
    var wrap = panel('Activity', h('div', null, [legend, host, note]));
    setTimeout(draw, 0);
    redraws.push(draw);
    return wrap;
  }
  function kindsPanel() {
    var kinds = logs.kinds || [];
    if (!kinds.length) { return panel('Query types', emptyNote('No statements.')); }
    var items = kinds.map(function (k) { return { label: k.kind, value: k.calls, color: KIND_COLORS[k.kind] || '#94a3b8' }; });
    var total = kinds.reduce(function (s, k) { return s + k.calls; }, 0);
    var legend = h('div', { 'class': 'donut-legend' }, kinds.map(function (k) {
      return h('div', null, [h('i', { style: { background: KIND_COLORS[k.kind] || '#94a3b8' } }), k.kind, h('span', { 'class': 'n', text: L.fmtInt(k.calls) + ' · ' + L.fmtPct(k.share_pct, 0) + ' time' })]);
    }));
    return panel('Query types', h('div', { 'class': 'donut-wrap' }, [C.donut(items, 'statements'), legend]), null);
  }
  function histogramPanel(title, buckets, color, total, note) {
    var count = buckets.reduce(function (s, b) { return s + b.count; }, 0);
    if (!count) { return panel(title, emptyNote('Nothing recorded.')); }
    return panel(title, h('div', null, [C.histogram(buckets, color), note ? h('p', { 'class': 'ladder', text: note }) : null, h('p', { 'class': 'muted session-note', text: 'Bar heights use a square-root scale so small ranges stay visible; counts are labelled and approximate near range edges.' })]));
  }
  function findingNode(f) {
    return h('div', { 'class': 'finding ' + f.severity }, [
      h('span', { 'class': 'dot' }),
      h('div', null, [
        h('div', null, [h('span', { 'class': 'code', text: f.code }), ' ', f.message]),
        f.statements && f.statements.length ? h('div', { 'class': 'links' }, f.statements.map(function (fp) {
          if (!byFp[fp]) { return h('span', { 'class': 'chip-off', title: 'Not among the statements shown (raise --top)', text: L.shortFp(fp) }); }
          return h('button', { title: 'Open the statement', onclick: function () { openStatement(fp); }, text: L.shortFp(fp) });
        })) : null
      ])
    ]);
  }

  /* ---------- statements ---------- */
  var COLUMNS = [
    { key: 'link', label: 'Source', cell: function (r) { return badge(r.link, r.link); } },
    { key: 'statement', label: 'Statement', cell: function (r) { return h('div', { 'class': 'stmt' }, [r.log && r.log.kind && r.log.kind !== 'other' ? h('span', { 'class': 'badge kind ' + r.log.kind, title: 'What the statement does', text: r.log.kind }) : null, h('span', { 'class': 'sql', text: r.text.replace(/\s+/g, ' ') }), h('span', { 'class': 'fp', text: L.shortFp(r.fingerprint) + (visibleFlags(r).length ? ' · ' + visibleFlags(r).join(', ') : '') })]); } },
    { key: 'log_calls', label: 'Log calls', num: true, cell: function (r) { return r.log ? L.fmtInt(r.log.calls) : '—'; } },
    { key: 'stats_calls', label: 'Stat calls', num: true, cell: function (r) { return r.stats ? L.fmtInt(r.stats.calls) : '—'; } },
    { key: 'coverage', label: 'Log / pgss calls', title: 'Calls the log recorded divided by calls pg_stat_statements counted', num: true, cell: function (r) {
      if (r.coverage === null || r.coverage === undefined) { return '—'; }
      var cls = r.coverage < 0.2 ? 'low' : r.coverage > 1.5 ? 'high' : '';
      return h('span', null, [h('span', { 'class': 'gauge ' + cls }, h('b', { style: { width: Math.min(100, r.coverage * 100) + '%' } })), L.fmtCoverage(r.coverage)]);
    } },
    { key: 'total', label: 'Total time', num: true, bar: true, cell: function (r) { return L.fmtMs(L.column(r, 'total')); } },
    { key: 'mean', label: 'Mean', num: true, cell: function (r) { return L.fmtMs(L.column(r, 'mean')); } },
    { key: 'p95', label: 'p95', num: true, cell: function (r) { return L.fmtMs(L.column(r, 'p95')); } },
    { key: 'max', label: 'Max', num: true, cell: function (r) { return L.fmtMs(L.column(r, 'max')); } },
    { key: 'share', label: 'Share', num: true, cell: function (r) { return L.fmtPct(L.column(r, 'share')); } },
    { key: 'errors', label: 'Errors', num: true, cell: function (r) { var e = L.column(r, 'errors'); return e === null ? '—' : L.fmtInt(e); } },
    { key: 'hit', label: 'Cache hit', title: 'Blocks found in shared buffers as a share of all blocks asked for', num: true, cell: function (r) { var v = L.column(r, 'hit'); return v === null || v === undefined ? '—' : L.fmtPct(v, 1); } },
    { key: 'io', label: 'I/O share', title: 'Time spent reading and writing files as a share of execution time (needs track_io_timing)', num: true, cell: function (r) { var v = L.column(r, 'io'); return v === null || v === undefined ? '—' : L.fmtPct(v, 0); } },
    { key: 'reads', label: 'Blocks read', title: 'Shared blocks read from disk or the OS cache', num: true, cell: function (r) { var v = L.column(r, 'reads'); return v === null ? '—' : L.fmtCompact(v); } },
    { key: 'rows_call', label: 'Rows/call', num: true, cell: function (r) { return fmtRatio(L.column(r, 'rows_call')); } },
    { key: 'blocks_row', label: 'Blocks read/row', title: 'High means scanning a lot to return little', num: true, cell: function (r) { return fmtRatio(L.column(r, 'blocks_row')); } },
    { key: 'variability', label: 'Spread', title: 'Standard deviation over the mean run time, since the statistics entry was created: high means uneven', num: true, cell: function (r) { var v = L.column(r, 'variability'); return v === null || v === undefined ? '\u2014' : v.toFixed(1) + '\u00d7'; } },
    { key: 'wal_call', label: 'WAL/call', num: true, cell: function (r) { var v = L.column(r, 'wal_call'); return v === null || v === undefined ? '—' : L.fmtBytes(v); } }
  ];
  function fmtRatio(v) { return v === null || v === undefined ? '—' : (v >= 100 ? L.fmtCompact(Math.round(v)) : v.toFixed(v >= 10 ? 1 : 2)); }
  /** What the "Top by" selector offers, and the column each choice sorts by. */
  var TOP_BY = [
    ['total', 'Total time'], ['calls', 'Calls'], ['mean', 'Mean time'], ['p95', 'p95 (log)'], ['max', 'Slowest (log)'],
    ['rows_call', 'Rows per call'], ['reads', 'Blocks read'], ['blocks_row', 'Blocks read per row'], ['wal_call', 'WAL per call'], ['io_ms', 'I/O time'], ['variability', 'Uneven run times']
  ];
  var hiddenCols = (function () {
    // What a reader of this report has no use for: log columns without a log, statistics
    // columns without statistics, and the detail columns until asked for.
    var fallback = ['max', 'errors', 'mean', 'stats_calls', 'reads', 'rows_call', 'blocks_row', 'wal_call', 'variability'];
    if (!logs) { fallback = ['log_calls', 'coverage', 'p95', 'max', 'errors', 'reads', 'rows_call', 'blocks_row', 'wal_call', 'variability']; }
    if (!stats) { fallback = fallback.concat(['hit', 'io', 'stats_calls']); }
    try {
      var saved = JSON.parse(store('pgloupe.cols.v2') || 'null');
      return Array.isArray(saved) ? saved.filter(function (k) { return typeof k === 'string'; }) : fallback;
    } catch (e) { return fallback; }
  })();
  var PAGE = 100;
  /** What the statements table keeps between redraws, so a live update does not reset it. */
  var ui = { shown: PAGE, cursor: -1 };

  function viewStatements() {
    var shown = ui.shown;
    var cursor = ui.cursor;
    var tbody = h('tbody');
    var count = h('span', { 'class': 'count' });
    var more = h('button', { 'class': 'btn more', onclick: function () { shown += PAGE; ui.shown = shown; refresh(); }, text: 'Show more' });
    var head = h('tr');
    var table = h('table', { 'class': 'cards' }, [h('thead', null, head), tbody]);
    var search = h('input', { 'class': 'search', type: 'search', placeholder: 'Search statements (press /)', 'aria-label': 'Search statements', value: state.q, id: 'stmt-search' });
    search.value = state.q;
    search.addEventListener('input', function () { state.q = search.value; shown = PAGE; ui.shown = shown; cursor = -1; ui.cursor = cursor; sync(); refresh(); });
    var chips = h('div', { 'class': 'toolbar' });
    var linkChips = ['all', 'both', 'log', 'stats'].filter(function (l) { return l === 'all' || rows.some(function (r) { return r.link === l; }); });
    var flagSet = {};
    rows.forEach(function (r) { visibleFlags(r).forEach(function (f) { flagSet[f] = true; }); });
    var flagChips = Object.keys(flagSet);
    var picker = h('div', { style: { position: 'relative' } });

    function sync() {
      var hash = L.buildHash(state);
      try { history.replaceState(null, '', hash || location.pathname + location.search); } catch (e) { /* the hash is a convenience */ }
    }
    function drawChips() {
      C.clear(chips);
      linkChips.forEach(function (l) {
        chips.appendChild(h('button', { 'class': 'chip', 'aria-pressed': String(state.link === l), onclick: function () { state.link = l; shown = PAGE; ui.shown = shown; sync(); drawChips(); refresh(); }, text: l === 'all' ? 'All' : l }));
      });
      if (state.kind) {
        chips.appendChild(h('button', { 'class': 'chip', 'aria-pressed': 'true', title: 'Clear the kind filter', onclick: function () { state.kind = ''; shown = PAGE; ui.shown = shown; sync(); drawChips(); refresh(); }, text: 'kind: ' + state.kind + ' \u00d7' }));
      }
      flagChips.forEach(function (f) {
        chips.appendChild(h('button', { 'class': 'chip', 'aria-pressed': String(state.flag === f), onclick: function () { state.flag = state.flag === f ? '' : f; shown = PAGE; ui.shown = shown; sync(); drawChips(); refresh(); }, text: f }));
      });
    }
    function drawHead() {
      C.clear(head);
      COLUMNS.forEach(function (c) {
        if (hiddenCols.indexOf(c.key) !== -1) { return; }
        var active = state.sort === c.key;
        head.appendChild(h('th', {
          title: c.title || null, 'class': (c.num ? 'num ' : '') + 'sortable', 'aria-sort': active ? (state.dir === 'asc' ? 'ascending' : 'descending') : 'none',
          onclick: function () { if (state.sort === c.key) { state.dir = state.dir === 'desc' ? 'asc' : 'desc'; } else { state.sort = c.key; state.dir = c.key === 'statement' || c.key === 'link' ? 'asc' : 'desc'; } sync(); drawHead(); refresh(); }
        }, [c.label, active ? h('span', { 'class': 'arrow', text: state.dir === 'asc' ? '▲' : '▼' }) : null]));
      });
    }
    function visible() {
      var filtered = L.filterStatements(rows, { q: state.q, link: state.link, flag: state.flag, kind: state.kind });
      return L.sortRows(filtered, function (r) { return L.column(r, state.sort); }, state.dir);
    }
    function refresh() {
      var all = visible();
      var page = all.slice(0, shown);
      var maxTotal = Math.max.apply(null, page.map(function (r) { return L.column(r, 'total') || 0; }).concat([1]));
      C.clear(tbody);
      page.forEach(function (r, index) {
        var tr = h('tr', { 'class': 'clickable' + (r.fingerprint === state.fp ? ' selected' : ''), 'data-fp': r.fingerprint, tabindex: '-1', onclick: function () { cursor = index; ui.cursor = cursor; openStatement(r.fingerprint); } });
        COLUMNS.forEach(function (c) {
          if (hiddenCols.indexOf(c.key) !== -1) { return; }
          var content = c.cell(r);
          if (c.bar) {
            var value = L.column(r, 'total') || 0;
            content = h('span', null, [h('span', { 'class': 'bar', style: { width: (value / maxTotal * 100).toFixed(1) + '%' } }), h('span', { text: content })]);
            tr.appendChild(h('td', { 'class': 'num bar-cell', 'data-label': c.label }, content));
          } else {
            tr.appendChild(h('td', { 'class': (c.num ? 'num' : '') + (c.key === 'statement' ? ' stmt-cell' : '') + (content === '\u2014' ? ' nil' : ''), 'data-label': c.label }, content));
          }
        });
        tbody.appendChild(tr);
      });
      count.textContent = all.length === rows.length ? rows.length + ' statements' : all.length + ' of ' + rows.length + ' statements';
      more.hidden = all.length <= shown;
      table.parentNode && (table.parentNode.hidden = all.length === 0);
      empty.hidden = all.length !== 0;
    }
    var empty = emptyNote('No statement matches. Clear the search or the filters.');
    var chooser = h('button', { 'class': 'btn small', 'aria-haspopup': 'true', onclick: function () {
      if (picker.querySelector('.popover')) { picker.querySelector('.popover').remove(); return; }
      var pop = h('div', { 'class': 'popover' }, COLUMNS.map(function (c) {
        var box = h('input', { type: 'checkbox' });
        box.checked = hiddenCols.indexOf(c.key) === -1;
        box.addEventListener('change', function () {
          hiddenCols = hiddenCols.filter(function (k) { return k !== c.key; });
          if (!box.checked) { hiddenCols.push(c.key); }
          store('pgloupe.cols.v2', JSON.stringify(hiddenCols)); drawHead(); refresh();
        });
        return h('label', null, [box, c.label]);
      }));
      picker.appendChild(pop);
    }, text: 'Columns' });
    picker.appendChild(chooser);
    var topBy = h('select', { 'aria-label': 'Top statements by', title: 'Rank the statements by' }, TOP_BY.map(function (o) {
      return h('option', { value: o[0], text: 'Top by: ' + o[1] });
    }));
    topBy.value = TOP_BY.some(function (o) { return o[0] === state.sort; }) ? state.sort : 'total';
    topBy.addEventListener('change', function () {
      state.sort = topBy.value; state.dir = 'desc';
      var column = { io_ms: 'io' }[topBy.value] || topBy.value;
      hiddenCols = hiddenCols.filter(function (k) { return k !== column; });
      store('pgloupe.cols.v2', JSON.stringify(hiddenCols));
      sync(); drawHead(); refresh();
    });
    drawChips(); drawHead();
    var wrap = h('div', { 'class': 'table-wrap' }, table);
    setTimeout(refresh, 0);
    viewHooks.statements = {
      move: function (delta) {
        var trs = tbody.querySelectorAll('tr');
        if (!trs.length) { return; }
        cursor = Math.max(0, Math.min(trs.length - 1, cursor + delta)); ui.cursor = cursor;
        trs.forEach(function (t) { t.classList.remove('cursor'); });
        trs[cursor].scrollIntoView({ block: 'nearest' });
        trs[cursor].style.outline = '2px solid var(--accent)';
        trs.forEach(function (t, i) { if (i !== cursor) { t.style.outline = ''; } });
      },
      open: function () { var trs = tbody.querySelectorAll('tr'); if (trs[cursor]) { openStatement(trs[cursor].getAttribute('data-fp')); } },
      focusSearch: function () { search.focus(); search.select(); }
    };
    return [
      h('div', { 'class': 'toolbar' }, [search, rows.some(function (r) { return r.stats; }) ? topBy : null, picker, count]),
      chips, wrap, empty,
      h('div', { 'class': 'toolbar', style: { 'justify-content': 'center' } }, more)
    ];
  }
  var viewHooks = {};

  function markSelected() {
    document.querySelectorAll('tr[data-fp]').forEach(function (tr) { tr.classList.toggle('selected', tr.getAttribute('data-fp') === state.fp); });
  }

  /* ---------- drawer ---------- */
  var scrim = h('div', { 'class': 'scrim', onclick: function () { setState({ fp: '' }); } });
  var drawerNode = h('aside', { 'class': 'drawer', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Statement details', 'aria-hidden': 'true' });
  function drawer() {
    var row = state.fp ? byFp[state.fp] : null;
    C.clear(drawerNode);
    if (!row) {
      drawerNode.classList.remove('open'); scrim.classList.remove('open'); drawerNode.setAttribute('aria-hidden', 'true');
      if (drawerCursor) { try { drawerCursor.focus(); } catch (e) { /* the row may be gone */ } drawerCursor = null; }
      return;
    }
    if (!drawerNode.classList.contains('open')) { drawerCursor = document.activeElement; }
    var close = h('button', { 'class': 'btn small close', onclick: function () { setState({ fp: '' }); }, 'aria-label': 'Close', text: 'Close (Esc)' });
    var body = h('div', { 'class': 'body' });
    body.appendChild(h('div', { 'class': 'toolbar' }, [
      h('button', { 'class': 'btn small', onclick: function () { copy(row.text, 'SQL'); }, text: 'Copy SQL' }),
      h('button', { 'class': 'btn small', onclick: function () { copy(row.fingerprint, 'Fingerprint'); }, text: 'Copy fingerprint' })
    ]));
    body.appendChild(sqlNode(row.text));
    var mine = findings.filter(function (f) { return f.statements && f.statements.indexOf(row.fingerprint) !== -1; });
    if (mine.length) { body.appendChild(h('div', { 'class': 'findings' }, mine.map(findingNode))); }
    var errs = errorsByFp[row.fingerprint] || [];
    if (errs.length) {
      body.appendChild(panel('Errors from this statement', h('div', { 'class': 'findings', id: 'drawer-errors' }, errs.map(function (x) {
        var e = x.error;
        return h('div', { 'class': 'finding' }, [h('span', { 'class': 'dot' }), h('div', null, [
          h('div', null, [badge(e.sqlstate || e.level, 'bad'), ' ' + L.fmtInt(x.count) + '\u00d7 this error kind', (e.statements || []).length > 1 ? ' (shared by ' + e.statements.length + ' statements)' : '']),
          h('div', { 'class': 'muted small', text: 'first of this kind: ' + e.first_message.split('\n')[0] }),
          e.detail ? h('div', { 'class': 'muted', text: e.detail }) : null,
          e.hint ? h('div', { 'class': 'muted', text: 'hint: ' + e.hint }) : null,
          e.pid ? h('div', { 'class': 'muted', text: 'first of this kind, from any statement: process ' + e.pid + ' at ' + L.fmtTime(e.first_seen, true) }) : null
        ])]);
      }))));
    }
    if (row.log && row.log.examples && row.log.examples.length) {
      body.appendChild(panel('Slowest run', simpleTable([{ label: 'Duration', num: true }, { label: 'When' }, { label: 'Who' }, { label: 'Parameters' }], row.log.examples.map(function (x) {
        return h('tr', null, [h('td', { 'class': 'num', text: L.fmtMs(x.duration_ms) }), h('td', { 'class': 'nowrap', text: L.fmtTime(x.timestamp, true) }),
          h('td', { text: [x.database, x.user, x.application, x.host].filter(Boolean).join(' / ') || '\u2014' }), h('td', { 'class': 'mono', text: x.params || '\u2014' })]);
      }))));
    }
    if (row.log) {
      var lg = row.log;
      body.appendChild(panel('What the log saw', h('div', null, [
        C.ladder(lg),
        facts([
          ['Calls', L.fmtInt(lg.calls)], ['With a duration', L.fmtInt(lg.timed_calls)], ['Total', L.fmtMs(lg.total_ms)], ['Mean', L.fmtMs(lg.mean_ms)],
          ['Min', L.fmtMs(lg.min_ms)], ['Max', L.fmtMs(lg.max_ms)], ['Errors', errs.length ? h('button', { 'class': 'linklike', onclick: function () { var t = document.getElementById('drawer-errors'); if (t) { t.scrollIntoView({ block: 'start' }); } }, text: L.fmtInt(lg.errors) + ' \u2191 see above' }) : L.fmtInt(lg.errors)], ['Lock waits', lg.lock_waits ? h('button', { 'class': 'linklike', onclick: function () { setState({ s: 'locks', fp: '' }); window.scrollTo(0, 0); }, text: L.fmtInt(lg.lock_waits) + ' (' + L.fmtMs(lg.lock_wait_ms) + ') \u2192 Locks' }) : '0'],
          ['Temp files', L.fmtInt(lg.temp_files) + (lg.temp_files ? ' (' + L.fmtBytes(lg.temp_bytes) + ')' : '')]].concat(lg.phases ? [
          ['Parse', L.fmtMs(lg.phases.parse.total_ms) + ' in ' + L.fmtInt(lg.phases.parse.count) + ' (max ' + L.fmtMs(lg.phases.parse.max_ms) + ')'],
          ['Bind', L.fmtMs(lg.phases.bind.total_ms) + ' in ' + L.fmtInt(lg.phases.bind.count) + ' (max ' + L.fmtMs(lg.phases.bind.max_ms) + ')'],
          ['Cursor fetches', L.fmtMs(lg.phases.fetch.total_ms) + ' in ' + L.fmtInt(lg.phases.fetch.count) + ' (max ' + L.fmtMs(lg.phases.fetch.max_ms) + ')'],
          ['Everything together', L.fmtMs(lg.all_ms)]
        ] : []).concat([['Simple / extended protocol', L.fmtInt(lg.statement_calls) + ' / ' + L.fmtInt(lg.execute_calls)],
          ['First seen', L.fmtTime(lg.first_seen, true)], ['Last seen', L.fmtTime(lg.last_seen, true)]
        ]))
      ])));
    }
    if (row.stats) {
      var st = row.stats;
      body.appendChild(panel('What pg_stat_statements saw', facts([
        ['Calls', L.fmtInt(st.calls)], ['Total', L.fmtMs(st.total_exec_ms)], ['Mean', L.fmtMs(st.mean_ms)], ['Share', L.fmtPct(st.share_pct)],
        ['Rows', L.fmtInt(st.rows)], st.plans ? ['Plans', L.fmtInt(st.plans)] : null, ['Shared hit', L.fmtInt(st.shared_blks_hit)], ['Shared read', L.fmtInt(st.shared_blks_read)],
        ['Temp read', L.fmtInt(st.temp_blks_read)], ['Temp written', L.fmtInt(st.temp_blks_written)], ['WAL', L.fmtBytes(st.wal_bytes)], st.nested_calls ? ['Nested calls', L.fmtInt(st.nested_calls)] : null,
        st.max_ms !== null && st.max_ms !== undefined ? ['Slowest run (since reset)', L.fmtMs(st.max_ms)] : null,
        st.variability !== null && st.variability !== undefined ? ['Spread (std dev / mean)', st.variability.toFixed(1) + '\u00d7'] : null,
        ['Entries', L.fmtInt(st.entries)], ['Query ids', st.queryids.join(', ') + (st.queryid_count > st.queryids.length ? ' (+' + (st.queryid_count - st.queryids.length) + ')' : '')],
        ['Flags', visibleFlags(row).length ? visibleFlags(row).join(', ') : 'none']
      ].filter(Boolean))));
    }
    if (row.coverage !== null && row.coverage !== undefined) {
      body.appendChild(panel('Coverage', h('p', { 'class': 'muted', text: 'The log recorded ' + L.fmtCoverage(row.coverage) + ' of the calls pg_stat_statements counted' + (row.query_id_matches === true ? '; the logged query_id matches.' : row.query_id_matches === false ? '; the logged query_id does not match.' : '.') })));
    }
    drawerNode.appendChild(h('div', { 'class': 'drawer-head' }, [badge(row.link, row.link), h('h2', { 'class': 'mono', text: L.shortFp(row.fingerprint) }), close]));
    drawerNode.appendChild(body);
    drawerNode.classList.add('open'); scrim.classList.add('open'); drawerNode.setAttribute('aria-hidden', 'false');
    close.focus();
  }
  function facts(pairs) {
    return h('dl', { 'class': 'facts' }, pairs.map(function (p) { return h('div', null, [h('dt', { text: p[0] }), h('dd', null, typeof p[1] === 'object' && p[1] !== null ? p[1] : String(p[1]))]); }));
  }

  /* ---------- other sections ---------- */
  function simpleTable(headers, bodyRows) {
    return h('div', { 'class': 'table-wrap' }, h('table', null, [
      h('thead', null, h('tr', null, headers.map(function (hd) { return h('th', { 'class': hd.num ? 'num' : '', text: hd.label }); }))),
      h('tbody', null, bodyRows)
    ]));
  }
  function viewErrors() {
    var out = [];
    var errs = logs.errors || [];
    var multiDay = logs.window && logs.window.first && logs.window.last && logs.window.first.slice(0, 10) !== logs.window.last.slice(0, 10);
    var max = Math.max.apply(null, errs.map(function (e) { return e.count; }).concat([1]));
    out.push(errs.length ? simpleTable(
      [{ label: 'Level' }, { label: 'SQLSTATE' }, { label: 'Count', num: true }, { label: 'What happened' }, { label: 'First seen' }, { label: 'Last seen' }],
      errs.map(function (e) {
        var chips = (e.statements || []).filter(function (s) { return byFp[s.fingerprint]; }).map(function (s) {
          return h('button', { 'class': 'linklike mono', title: 'Open the statement', onclick: function () { openStatement(s.fingerprint); }, text: L.shortFp(s.fingerprint) + (s.count > 1 ? ' \u00d7' + s.count : '') });
        });
        return h('tr', null, [
          h('td', null, badge(e.level, e.level === 'ERROR' ? 'bad' : 'warn')), h('td', { 'class': 'mono', text: e.sqlstate || '\u2014' }),
          h('td', { 'class': 'num bar-cell' }, [h('span', { 'class': 'bar', style: { width: (e.count / max * 100) + '%' } }), h('span', { text: L.fmtInt(e.count) })]),
          h('td', null, [h('div', { text: e.first_message.split('\n')[0] }),
            e.detail ? h('div', { 'class': 'muted small', text: e.detail }) : null, e.hint ? h('div', { 'class': 'muted small', text: 'hint: ' + e.hint }) : null,
            e.pid ? h('div', { 'class': 'muted small', text: 'process ' + e.pid }) : null,
            chips.length ? h('div', { 'class': 'links' }, [h('span', { 'class': 'muted small', text: 'statements' }), chips]) : null]),
          h('td', { 'class': 'nowrap', text: L.fmtTime(e.first_seen, multiDay) }), h('td', { 'class': 'nowrap', text: L.fmtTime(e.last_seen, multiDay) })
        ]);
      })) : panel('Errors', emptyNote('No errors in the logs.')));
    var c = logs.cancelled;
    if (c && c.total) {
      var maxReason = Math.max.apply(null, c.by_reason.map(function (r) { return r.count; }));
      var reasons = h('div', { 'class': 'ranked' }, c.by_reason.map(function (r) {
        return h('div', { 'class': 'row' }, [h('span', { 'class': 'name', text: r.reason.replace('_', ' ') }), h('span', { 'class': 'track' }, h('b', { style: { width: (r.count / maxReason * 100) + '%' } })), h('span', { 'class': 'val', text: L.fmtInt(r.count) })]);
      }));
      var list = simpleTable([{ label: 'Statement' }, { label: 'Cancelled', num: true }, { label: 'Calls', num: true }], c.statements.map(function (s) {
        var known = byFp[s.fingerprint];
        return h('tr', { 'class': known ? 'clickable' : '', onclick: known ? function () { openStatement(s.fingerprint); } : null }, [
          h('td', null, h('div', { 'class': 'stmt' }, [h('span', { 'class': 'sql', text: s.text.replace(/\s+/g, ' ') }), h('span', { 'class': 'fp', text: L.shortFp(s.fingerprint) })])),
          h('td', { 'class': 'num', text: L.fmtInt(s.cancelled) }), h('td', { 'class': 'num', title: 'Cancelled executions are not counted as calls', text: s.calls ? L.fmtInt(s.calls) : '\u2014' })]);
      }));
      out.push(h('div', { 'class': 'grid cols-2' }, [panel('Cancelled by reason', reasons), panel('Cancelled most often', list)]));
    }
    var chatter = logs.messages || [];
    if (chatter.length) {
      out.push(h('h2', { text: 'Most frequent other messages' }));
      out.push(h('p', { 'class': 'muted', text: 'Messages the server logs that are not statements, errors or events pgloupe knows, grouped with their numbers and quoted text taken out. A message that repeats thousands of times is usually application code logging on purpose.' }));
      out.push(simpleTable([{ label: 'Times', num: true }, { label: 'First seen' }, { label: 'Message' }], chatter.map(function (m) {
        return h('tr', null, [h('td', { 'class': 'num', text: L.fmtInt(m.count) }), h('td', { 'class': 'nowrap', text: L.fmtTime(m.first_seen, true) }), h('td', { 'class': 'mono', title: m.example, text: m.shape })]);
      })));
    }
    return out;
  }
  function viewLocks() {
    var l = logs.locks, t = logs.temp_files;
    var out = [h('div', { 'class': 'grid kpis' }, [
      kpi('Lock waits', L.fmtInt(l.waits), l.acquired + ' completed after waiting'), kpi('Total wait', L.fmtMs(l.total_wait_ms), 'longest ' + L.fmtMs(l.max_wait_ms)),
      kpi('Deadlocks', L.fmtInt(l.deadlocks), '', null, l.deadlocks ? 'bad' : ''), kpi('Temp files', L.fmtInt(t.files), L.fmtBytes(t.bytes) + ' total'), kpi('Largest temp file', L.fmtBytes(t.max_bytes), '')
    ])];
    if (display.some(function (b) { return b.lock_waits > 0; })) { out.push(chartPanel('Lock waits over time', ['lock_waits'])); }
    else if (notLogged('lock waits')) { out.push(panel('Lock waits over time', emptyNote('Not logged: log_lock_waits is off, or no lock wait lasted longer than deadlock_timeout.'))); }
    var waiters = rows.filter(function (r) { return r.log && r.log.lock_waits > 0; }).sort(function (a, b) { return b.log.lock_wait_ms - a.log.lock_wait_ms; }).slice(0, 15);
    if (waiters.length) {
      out.push(h('h2', { text: 'Statements that waited for locks' }));
      out.push(simpleTable([{ label: 'Statement' }, { label: 'Waits', num: true }, { label: 'Wait time', num: true }], waiters.map(function (r) {
        return h('tr', { 'class': 'clickable', onclick: function () { openStatement(r.fingerprint); } }, [h('td', null, h('div', { 'class': 'stmt' }, h('span', { 'class': 'sql', text: r.text.replace(/\s+/g, ' ') }))), h('td', { 'class': 'num', text: L.fmtInt(r.log.lock_waits) }), h('td', { 'class': 'num', text: L.fmtMs(r.log.lock_wait_ms) })]);
      })));
    }
    if (l.blockers && l.blockers.length) {
      out.push(h('h2', { text: 'Who blocks whom' }));
      out.push(h('p', { 'class': 'muted', text: 'The holder is the statement the blocking backend was running, or had last run while idle in its transaction. Waits are the times the log reported, at least that long.' }));
      var holder = function (fp, text) {
        return h('td', null, fp && byFp[fp] ? h('button', { 'class': 'linklike', onclick: function () { openStatement(fp); }, text: text.replace(/\s+/g, ' ').slice(0, 140) }) : h('span', { 'class': fp ? 'sql' : 'muted', text: fp ? text.replace(/\s+/g, ' ').slice(0, 140) : '(not in the log)' }));
      };
      out.push(simpleTable([{ label: 'Times', num: true }, { label: 'Waited', num: true }, { label: 'Holder' }, { label: 'Waiter' }], l.blockers.map(function (b) {
        return h('tr', null, [h('td', { 'class': 'num', text: L.fmtInt(b.count) }), h('td', { 'class': 'num', text: L.fmtMs(b.total_wait_ms) }), holder(b.blocker, b.blocker_text), holder(b.waiter, b.waiter_text)]);
      })));
    }
    var temps = rows.filter(function (r) { return r.log && r.log.temp_files > 0; }).sort(function (a, b) { return b.log.temp_bytes - a.log.temp_bytes; }).slice(0, 15);
    if (temps.length) {
      out.push(h('h2', { text: 'Statements that wrote temp files' }));
      out.push(simpleTable([{ label: 'Statement' }, { label: 'Files', num: true }, { label: 'Size', num: true }], temps.map(function (r) {
        return h('tr', { 'class': 'clickable', onclick: function () { openStatement(r.fingerprint); } }, [h('td', null, h('div', { 'class': 'stmt' }, h('span', { 'class': 'sql', text: r.text.replace(/\s+/g, ' ') }))), h('td', { 'class': 'num', text: L.fmtInt(r.log.temp_files) }), h('td', { 'class': 'num', text: L.fmtBytes(r.log.temp_bytes) })]);
      })));
    }
    return out;
  }
  function viewSessions() {
    var c = logs.connections, q = c.session_ms;
    var logged = !notLogged('connections') || c.received || c.disconnections;
    var kpis = logged ? [
      kpi('Connections', L.fmtInt(c.authorized), c.received === c.authorized && c.authenticated === c.authorized ? 'received, authenticated and authorized' : 'received ' + L.fmtInt(c.received) + ' \u00b7 authenticated ' + L.fmtInt(c.authenticated)),
      kpi('Disconnections', L.fmtInt(c.disconnections), ''),
      kpi('Median session', L.fmtMs(q.p50), q.count ? 'p95 ' + L.fmtMs(q.p95) + ', longest ' + L.fmtMs(q.max) : 'no sessions ended')
    ] : [offKpi('Connections', 'log_connections and log_disconnections are off')];
    var db = D.database && D.database.total;
    if (db) {
      kpis.push(kpi('Sessions started', L.fmtInt(db.sessions), 'from pg_stat_database'));
      kpis.push(kpi('Abandoned', pctText(db.abandoned_pct), L.fmtInt(db.sessions_abandoned) + ' ended without a clean goodbye', null, db.abandoned_pct >= 20 && db.sessions_abandoned >= 50 ? 'warn' : ''));
      kpis.push(kpi('Killed or fatal', L.fmtInt(db.sessions_killed + db.sessions_fatal), L.fmtInt(db.sessions_killed) + ' killed, ' + L.fmtInt(db.sessions_fatal) + ' fatal'));
    }
    var out = [h('div', { 'class': 'grid kpis' }, kpis)];
    if (display.some(function (b) { return b.connections > 0 || b.aborts > 0; })) { out.push(chartPanel('Sessions over time', ['connections', 'aborts'])); }
    if (logged && q.count) { out.push(histogramPanel('Session durations', logs.histograms.sessions, 'var(--c-connections)')); }
    return out.concat(hostPanel(), abruptPanel());
  }
  function hostPanel() {
    var series = logs.connection_hosts;
    if (!series || !series.hosts.length || !series.buckets.length) { return []; }
    var palette = ['#4f7cff', '#12a594', '#f0a020', '#e5484d', '#8b5cf6', '#d946ef', '#64748b', '#0ea5e9', '#94a3b8'];
    var rows2 = series.buckets.map(function (b) {
      var row = { start: b.start };
      b.counts.forEach(function (n, i) { row['h' + i] = n; });
      return row;
    });
    var cells = L.downsample(rows2, 240);
    var host = h('div', { 'class': 'chart-host' });
    setTimeout(function () {
      C.timeline(host, { buckets: cells, series: series.hosts.map(function (name, i) { return { key: 'h' + i, label: name, color: palette[i % palette.length] }; }), range: null });
    }, 0);
    return [panel('Connections by host', host)];
  }
  function abruptPanel() {
    var d = logs.disconnects;
    if (!d || !d.reasons.length) { return []; }
    var names = {
      client_reset: 'client reset the connection', client_gone: 'client vanished', client_gone_in_transaction: 'client vanished with a transaction open',
      client_unreachable: 'could not send to the client', client_lost: 'connection to the client lost', idle_in_transaction_timeout: 'idle in transaction timeout',
      idle_session_timeout: 'idle session timeout', terminated_by_admin: 'ended by an administrator', recovery_conflict: 'conflict with recovery',
      protocol_error: 'protocol error', authentication_timeout: 'authentication timed out'
    };
    var out = [h('h2', { text: 'Sessions that ended abruptly' }), h('p', { 'class': 'muted', text: 'Idle time per session is not in the log; these are the endings the server reports.' })];
    out.push(simpleTable([{ label: 'Reason' }, { label: 'Times', num: true }], d.reasons.map(function (r) {
      return h('tr', null, [h('td', { text: names[r.reason] || r.reason }), h('td', { 'class': 'num', text: L.fmtInt(r.count) })]);
    })));
    out.push(simpleTable([{ label: 'Application' }, { label: 'Reason' }, { label: 'Times', num: true }], d.by_application.slice(0, 15).map(function (r) {
      return h('tr', null, [h('td', { text: r.application }), h('td', { text: names[r.reason] || r.reason }), h('td', { 'class': 'num', text: L.fmtInt(r.count) })]);
    })));
    return out;
  }
  function viewMaintenance() {
    var out = [];
    var checkpointTable = logs.checkpoints.length ? simpleTable(
      [{ label: 'Kind' }, { label: 'Started', num: true }, { label: 'Completed', num: true }, { label: 'Reasons' }, { label: 'Buffers', num: true }, { label: 'WAL between (mean / max)', num: true }, { label: 'Sync p95', num: true }, { label: 'Write p95', num: true }, { label: 'Total p95', num: true }],
      logs.checkpoints.map(function (c) {
        return h('tr', null, [h('td', { text: c.kind }), h('td', { 'class': 'num', text: L.fmtInt(c.started) }), h('td', { 'class': 'num', text: L.fmtInt(c.completed) }),
          h('td', null, Object.keys(c.reasons).map(function (k) { return badge(k + ' ×' + c.reasons[k]); })), h('td', { 'class': 'num', text: L.fmtInt(c.buffers) }),
          h('td', { 'class': 'num', text: c.distance_kb ? L.fmtBytes(c.distance_kb.mean_kb * 1024) + ' / ' + L.fmtBytes(c.distance_kb.max_kb * 1024) : '\u2014' }),
          h('td', { 'class': 'num', text: L.fmtMs(c.sync_ms.p95) }), h('td', { 'class': 'num', text: L.fmtMs(c.write_ms.p95) }), h('td', { 'class': 'num', text: L.fmtMs(c.total_ms.p95) })]);
      })) : panel('Checkpoints', emptyNote('No checkpoints were logged. Set log_checkpoints = on.'));
    out.push(checkpointTable);
    if (logs.checkpoints.length) { out.push(h('p', { 'class': 'muted', text: 'Sync time is the part that can hurt: it is when the checkpoint waits for the disk. A write time that is the same for every checkpoint is not a problem; the server spreads the writes over checkpoint_timeout times checkpoint_completion_target on purpose.' })); }
    var av = logs.autovacuum;
    var maxAv = Math.max.apply(null, av.map(function (t) { return t.total_ms; }).concat([1]));
    out.push(av.length ? simpleTable([{ label: 'Table' }, { label: 'Vacuums', num: true }, { label: 'Analyzes', num: true }, { label: 'Total time', num: true }, { label: 'Longest', num: true }, { label: 'Tuples removed', num: true }, { label: 'Pages removed', num: true }],
      av.map(function (t) {
        return h('tr', null, [h('td', { 'class': 'mono', text: t.table }), h('td', { 'class': 'num', text: L.fmtInt(t.vacuums) }), h('td', { 'class': 'num', text: L.fmtInt(t.analyzes) }),
          h('td', { 'class': 'num bar-cell' }, [h('span', { 'class': 'bar', style: { width: (t.total_ms / maxAv * 100) + '%' } }), h('span', { text: L.fmtMs(t.total_ms) })]), h('td', { 'class': 'num', text: L.fmtMs(t.max_ms) }), h('td', { 'class': 'num', text: L.fmtInt(t.tuples_removed) }), h('td', { 'class': 'num', text: L.fmtInt(t.pages_removed) })]);
      })) : panel('Autovacuum', emptyNote('No autovacuum runs were logged (log_autovacuum_min_duration).')));
    if (display.some(function (b) { return b.checkpoints || b.wal_kb || b.ddl; })) { out.push(chartPanel('Maintenance over time', ['wal_kb', 'checkpoints', 'ddl'])); }
    var schema = logs.ddl || { tags: [], temp_tables: [] };
    if (schema.tags.length) {
      out.push(h('h2', { text: 'Schema changes' }));
      out.push(simpleTable([{ label: 'Command' }, { label: 'Times', num: true }, { label: 'Total time', num: true }], schema.tags.map(function (t) {
        return h('tr', null, [h('td', { 'class': 'mono', text: t.tag }), h('td', { 'class': 'num', text: L.fmtInt(t.calls) }), h('td', { 'class': 'num', text: L.fmtMs(t.total_ms) })]);
      })));
      if (schema.temp_tables.length) {
        out.push(h('p', { 'class': 'muted', text: 'Temporary tables are created and dropped in the system catalogs; many of them bloat pg_class and pg_attribute.' }));
        out.push(simpleTable([{ label: 'Application' }, { label: 'Temporary tables created', num: true }], schema.temp_tables.map(function (t) {
          return h('tr', null, [h('td', { text: t.name }), h('td', { 'class': 'num', text: L.fmtInt(t.count) })]);
        })));
      }
    }
    return out;
  }
  var breakdownMetric = 'time';
  var breakdownAll = false;
  function statementRollups() {
    if (!stats || !stats.by_database || !stats.by_database.length) { return []; }
    var ranked = function (title, list) {
      var max = Math.max.apply(null, list.map(function (r) { return r.total_ms; }).concat([1]));
      return panel(title, h('div', { 'class': 'ranked' }, list.map(function (r) {
        return h('div', { 'class': 'row', title: r.name }, [h('span', { 'class': 'name', text: r.name }), h('span', { 'class': 'track' }, h('b', { style: { width: (r.total_ms / max * 100) + '%' } })),
          h('span', { 'class': 'val', text: L.fmtMs(r.total_ms) + ' \u00b7 ' + L.fmtPct(r.share_pct, 0) + ' \u00b7 ' + L.fmtCompact(r.calls) + ' calls' })]);
      })));
    };
    return [h('h2', { text: 'From pg_stat_statements' }), h('p', { 'class': 'muted', text: 'Execution time in the window, by the database and role that ran the statements. These do not depend on what the log recorded.' }),
      h('div', { 'class': 'grid cols-2' }, [ranked('Databases', stats.by_database), ranked('Roles', stats.by_user)])];
  }
  function viewBreakdowns() {
    var wrap = h('div', { 'class': 'grid cols-2' });
    var chips = h('div', { 'class': 'toolbar' });
    var skipped = h('p', { 'class': 'muted' });
    function metricOf(r) { return breakdownMetric === 'time' ? r.total_ms : breakdownMetric === 'errors' ? r.errors : breakdownMetric === 'connections' ? r.connections : r.statements; }
    function draw() {
      C.clear(chips); C.clear(wrap); skipped.textContent = '';
      var absent = [];
      [['time', 'Statement time'], ['statements', 'Statements'], ['errors', 'Errors'], ['connections', 'Connections']].forEach(function (m) {
        chips.appendChild(h('button', { 'class': 'chip', 'aria-pressed': String(breakdownMetric === m[0]), onclick: function () { breakdownMetric = m[0]; draw(); }, text: m[1] }));
      });
      chips.appendChild(h('button', { 'class': 'chip', 'aria-pressed': String(breakdownAll), onclick: function () { breakdownAll = !breakdownAll; draw(); }, text: 'Show empty rows' }));
      [['database', 'Databases'], ['user', 'Users'], ['application', 'Applications'], ['host', 'Hosts']].forEach(function (d) {
        var all = logs.dimensions[d[0]] || [];
        // A dimension whose only value is "(none)" is a column the log never carried.
        if (!all.some(function (r) { return r.value !== '(none)'; })) { absent.push(d[1].toLowerCase()); return; }
        var list = all.filter(function (r) { return breakdownAll || metricOf(r) > 0; }).sort(function (a, b) { return metricOf(b) - metricOf(a); });
        var max = Math.max.apply(null, list.map(metricOf).concat([1]));
        wrap.appendChild(panel(d[1], list.length ? h('div', { 'class': 'ranked' }, list.map(function (r) {
          return h('div', { 'class': 'row', title: r.value }, [h('span', { 'class': 'name', text: r.value }), h('span', { 'class': 'track' }, h('b', { style: { width: (metricOf(r) / max * 100) + '%' } })),
            h('span', { 'class': 'val', text: breakdownMetric === 'time' ? L.fmtMs(r.total_ms) : L.fmtInt(metricOf(r)) + ' \u00b7 ' + L.fmtInt(r.statements) + ' stmts' })]);
        })) : emptyNote('Nothing recorded.')));
      });
      if (absent.length) { skipped.textContent = 'Not in this log: ' + absent.join(', ') + ' (the log_line_prefix does not carry them, or nothing was timed).'; }
    }
    if (!logs) { return statementRollups(); }
    draw();
    return [chips, wrap, skipped].concat(statementRollups());
  }
  function viewSlowest() {
    var s = logs.slowest;
    var out = [];
    if (!s.has_params) { out.push(h('p', { 'class': 'muted', text: 'No row carries bind parameters. They are kept only with --params (when analyzing or ingesting), and only extended-protocol statements log them; they can hold personal data.' })); }
    var order = [], groups = Object.create(null);
    s.rows.forEach(function (r) { if (!groups[r.fingerprint]) { groups[r.fingerprint] = []; order.push(r.fingerprint); } groups[r.fingerprint].push(r); });
    order.forEach(function (fp) {
      var runs = groups[fp], r = runs[0];
      var who = [r.database, r.user, r.application, r.host].filter(Boolean).join(' / ');
      var sql = sqlNode(r.text);
      sql.style.maxHeight = '110px';
      out.push(h('div', { 'class': 'panel slow' }, [
        h('div', { 'class': 'head' }, [h('span', { 'class': 'ms', text: L.fmtMs(r.duration_ms) }), h('span', { 'class': 'who', text: L.fmtTime(r.timestamp, true) + (who ? ' \u00b7 ' + who : '') }),
          byFp[fp] ? h('button', { 'class': 'btn small', onclick: function () { openStatement(fp); }, text: 'Statement ' + L.shortFp(fp) }) : null,
          h('button', { 'class': 'btn small', onclick: function () { sql.style.maxHeight = sql.style.maxHeight ? '' : '110px'; }, text: 'Full SQL' })]),
        sql, r.params ? h('div', { 'class': 'params', text: 'parameters: ' + r.params }) : null,
        runs.length > 1 ? h('div', { 'class': 'muted small', text: 'also slow: ' + runs.slice(1, 5).map(function (x) { return L.fmtMs(x.duration_ms) + ' at ' + L.fmtTime(x.timestamp) + (x.user ? ' (' + x.user + ')' : ''); }).join(' \u00b7 ') + (runs.length > 5 ? ' \u00b7 +' + (runs.length - 5) + ' more' : '') }) : null
      ]));
    });
    return out;
  }
  function viewComparison() {
    var c = D.comparison;
    var out = [];
    var change = function (p) {
      if (!p.before) { return p.now ? 'new' : '\u2014'; }
      var pct = (p.now - p.before) / p.before * 100;
      return (pct >= 0 ? '+' : '') + Math.round(pct) + '%';
    };
    var tone = function (p, worseWhenUp) {
      if (!p.before || p.now === p.before) { return ''; }
      return (p.now > p.before) === worseWhenUp ? 'warn' : 'good';
    };
    out.push(h('p', { 'class': 'muted', text: 'This period against ' + L.fmtTime(c.baseline.since, true) + ' \u2192 ' + L.fmtTime(c.baseline.until, true) + '. ' + c.new_statements + ' statements are new, ' + c.gone_statements + ' are gone.' }));
    var t = c.totals;
    out.push(h('div', { 'class': 'kpis' }, [
      kpi('Statement time', L.fmtMs(t.total_ms.now), 'before ' + L.fmtMs(t.total_ms.before) + ' \u00B7 ' + change(t.total_ms), null, tone(t.total_ms, true)),
      kpi('Statements run', L.fmtInt(t.statements.now), 'before ' + L.fmtInt(t.statements.before) + ' \u00B7 ' + change(t.statements)),
      kpi('Errors', L.fmtInt(t.errors.now), 'before ' + L.fmtInt(t.errors.before) + ' \u00B7 ' + change(t.errors), null, tone(t.errors, true)),
      kpi('Lock waits', L.fmtInt(t.lock_waits.now), 'before ' + L.fmtInt(t.lock_waits.before) + ' \u00B7 ' + change(t.lock_waits), null, tone(t.lock_waits, true)),
      kpi('Deadlocks', L.fmtInt(t.deadlocks.now), 'before ' + L.fmtInt(t.deadlocks.before), null, tone(t.deadlocks, true)),
      kpi('Temp files', L.fmtBytes(t.temp_bytes.now), 'before ' + L.fmtBytes(t.temp_bytes.before), null, tone(t.temp_bytes, true))
    ]));
    var rows = c.statements.map(function (m) {
      var tr = h('tr', { 'class': byFp[m.fingerprint] ? 'clickable' : '', onclick: function () { if (byFp[m.fingerprint]) { openStatement(m.fingerprint); } } }, [
        h('td', null, badge(m.status, m.status === 'slower' || m.status === 'new' ? 'warn' : m.status === 'faster' ? 'info' : '')),
        h('td', { 'class': 'num', text: L.fmtInt(m.calls.before) + ' \u2192 ' + L.fmtInt(m.calls.now) }),
        h('td', { 'class': 'num', text: m.mean_ms ? L.fmtMs(m.mean_ms.before) + ' \u2192 ' + L.fmtMs(m.mean_ms.now) : '\u2014' }),
        h('td', { 'class': 'num', text: L.fmtMs(m.total_ms.before) + ' \u2192 ' + L.fmtMs(m.total_ms.now) }),
        h('td', { 'class': 'num', text: m.total_change_pct === null || m.total_change_pct === undefined ? 'new' : (m.total_change_pct >= 0 ? '+' : '') + Math.round(m.total_change_pct) + '%' }),
        h('td', { 'class': 'mono', text: m.text.length > 140 ? m.text.slice(0, 139) + '\u2026' : m.text })
      ]);
      return tr;
    });
    out.push(panel('What moved most', simpleTable([{ label: 'Change' }, { label: 'Calls', num: true }, { label: 'Mean', num: true }, { label: 'Total time', num: true }, { label: '%', num: true }, { label: 'Statement' }], rows)));
    return out;
  }
  function viewPooler() {
    var p = logs.pooler;
    var cells = L.downsample(p.points.map(function (x) {
      return { start: x.start, xacts_s: x.xacts_s, queries_s: x.queries_s, wait_ms: x.wait_ms };
    }), 240).map(function (b) {
      // Rates and times are averages, not sums: undo the summing a merged bar did.
      var n = b.span || 1;
      return { start: b.start, span: n, xacts_s: b.xacts_s / n, queries_s: b.queries_s / n, wait_ms: b.wait_ms / n };
    });
    var host = h('div', { 'class': 'chart-host' });
    setTimeout(function () {
      C.timeline(host, { buckets: cells, series: [{ key: 'xacts_s', label: 'Transactions / s', color: 'var(--c-statements)' }, { key: 'queries_s', label: 'Queries / s', color: 'var(--c-connections)' }, { key: 'wait_ms', label: 'Client wait (ms)', color: 'var(--c-locks)' }], range: null });
    }, 0);
    var out = [
      h('p', { 'class': 'muted', text: 'Read from the pgBouncer log (experimental: not yet confirmed against a real pooler).' }),
      h('div', { 'class': 'grid kpis' }, [kpi('Logins', L.fmtInt(p.logins), ''), kpi('Peak transactions / s', L.fmtInt(Math.round(p.peak_xacts_s)), ''), kpi('Mean client wait', L.fmtMs(p.mean_wait_ms), 'for a server connection')]),
      panel('Pooler activity', host)
    ];
    if (p.close_reasons.length) {
      out.push(h('h2', { text: 'Why sessions ended' }));
      out.push(simpleTable([{ label: 'Reason' }, { label: 'Times', num: true }], p.close_reasons.map(function (r) {
        return h('tr', null, [h('td', { text: r.reason }), h('td', { 'class': 'num', text: L.fmtInt(r.count) })]);
      })));
    }
    return out;
  }
  function viewAdvice() {
    var out = [h('p', { 'class': 'muted', text: 'Settings that decide what this report can show. Each one says what is lost and what to change; the badge says whether it was read from the server\'s settings or inferred from the data.' })];
    if (advice.length && advice.every(function (a) { return a.source !== 'settings'; })) {
      out.push(h('div', { 'class': 'finding info' }, [h('span', { 'class': 'dot' }), h('div', { text: 'All of this is inferred from the data. Give the server\'s real settings and it is replaced by what they say: pass --settings FILE (a CSV of name,setting), or take snapshots with snap, which writes settings_<time>.csv beside each one and is picked up automatically.' })]));
    }
    advice.forEach(function (a) {
      out.push(h('div', { 'class': 'finding ' + a.severity }, [h('span', { 'class': 'dot' }), h('div', null, [
        h('div', null, [h('code', { text: a.code }), ' ', badge(a.source === 'settings' ? 'from the settings' : 'from the data', a.source === 'settings' ? 'info' : '')]),
        h('div', { text: a.message })])]));
    });
    return out;
  }
  function viewFindings() { return [h('div', { 'class': 'findings' }, findings.map(findingNode))]; }

  var VIEWS = { overview: viewOverview, statements: viewStatements, errors: viewErrors, locks: viewLocks, sessions: viewSessions, maintenance: viewMaintenance, breakdowns: viewBreakdowns, slowest: viewSlowest, pooler: viewPooler, comparison: viewComparison, advice: viewAdvice, findings: viewFindings , sources: viewSources };

  /* ---------- render ---------- */
  var viewNode = null;
  function render() {
    redraws = [];
    renderChrome();
    var mainNode = root.querySelector('.main');
    var next = h('div', { 'class': 'view' }, (VIEWS[state.s] || viewOverview)());
    if (viewNode && viewNode.parentNode) { viewNode.parentNode.replaceChild(next, viewNode); } else { mainNode.appendChild(next); }
    viewNode = next;
    drawer();
  }

  document.addEventListener('keydown', function (event) {
    if (event.key === 'Tab' && state.fp && drawerNode.classList.contains('open')) {
      var items = drawerNode.querySelectorAll('button, [href], input, [tabindex]:not([tabindex="-1"])');
      if (items.length) {
        var first = items[0], last = items[items.length - 1];
        if (!drawerNode.contains(document.activeElement)) { event.preventDefault(); first.focus(); }
        else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }
      return;
    }
    var tag = (event.target && event.target.tagName) || '';
    var typing = tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
    if (event.key === 'Escape') {
      if (state.fp) { setState({ fp: '' }); }
      else if (typing) { event.target.blur(); }
      return;
    }
    if (typing || event.ctrlKey || event.metaKey || event.altKey) { return; }
    if (event.key === '/') {
      event.preventDefault();
      if (state.s !== 'statements') { setState({ s: 'statements' }); }
      setTimeout(function () { var s = document.getElementById('stmt-search'); if (s) { s.focus(); s.select(); } }, 0);
    } else if (event.key === 'j' || event.key === 'k') {
      if (state.s === 'statements' && viewHooks.statements && !state.fp) { viewHooks.statements.move(event.key === 'j' ? 1 : -1); }
    } else if (event.key === 'Enter') {
      if (tag === 'BUTTON' || tag === 'A') { return; }
      if (state.s === 'statements' && viewHooks.statements && !state.fp) { viewHooks.statements.open(); }
    } else if (/^[1-9]$/.test(event.key)) {
      var target = sections[Number(event.key) - 1];
      if (target) { setState({ s: target.id, fp: '' }); window.scrollTo(0, 0); }
    }
  });
  var resizeTimer = null;
  window.addEventListener('resize', function () {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(function () { redraws.forEach(function (draw) { draw(); }); }, 150);
  });
  window.addEventListener('hashchange', function () { state = L.parseHash(location.hash); if (!sections.some(function (s) { return s.id === state.s; })) { state.s = 'overview'; } render(); });

  window.addEventListener('beforeprint', function () { document.documentElement.classList.remove('app'); });
  window.addEventListener('afterprint', function () { document.documentElement.classList.add('app'); });
  document.documentElement.classList.add('app');
  applyTheme();
  document.body.appendChild(scrim);
  document.body.appendChild(drawerNode);
  render();
  /** Lets the live updater swap in a newer report without losing the view. */
  window.PgApp = {
    update: function (raw) {
      var y = window.scrollY;
      var oldDisplay = display, held = rangeNow();
      var typing = document.activeElement && document.activeElement.id === 'stmt-search' ? { from: document.activeElement.selectionStart, to: document.activeElement.selectionEnd } : null;
      var drawerTop = drawerNode.scrollTop;
      if (!load(raw)) { return false; }
      // Selections are bar indices, and the bars slide as the window moves: keep the same time.
      var moved = L.remapRange(oldDisplay, held, display);
      state.from = moved ? moved.from : null;
      state.to = moved ? moved.to : null;
      try { history.replaceState(null, '', L.buildHash(state) || location.pathname + location.search); } catch (e) { /* the hash is a convenience */ }
      render();
      window.scrollTo(0, y);
      drawerNode.scrollTop = drawerTop;
      if (typing) {
        var box = document.getElementById('stmt-search');
        if (box) { box.focus(); try { box.setSelectionRange(typing.from, typing.to); } catch (e) { /* not every input type has a selection */ } }
      }
      return true;
    }
  };
})();
