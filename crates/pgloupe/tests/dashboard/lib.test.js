const test = require('node:test');
const assert = require('node:assert/strict');
const L = require('../../src/combined/templates/lib.js');

test('numbers are formatted with units that read well', () => {
  assert.equal(L.fmtInt(1234567), '1,234,567');
  assert.equal(L.fmtInt(0), '0');
  assert.equal(L.fmtInt(null), '—');
  assert.equal(L.fmtMs(0.1234), '0.123 ms');
  assert.equal(L.fmtMs(12.34), '12.3 ms');
  assert.equal(L.fmtMs(1500), '1.50 s');
  assert.equal(L.fmtMs(90000), '1.50 min');
  assert.equal(L.fmtMs(7200000), '2.00 h');
  assert.equal(L.fmtMs(undefined), '—');
  assert.equal(L.fmtBytes(512), '512 B');
  assert.equal(L.fmtBytes(1536), '1.5 KiB');
  assert.equal(L.fmtCompact(1250000), '1.3M');
  assert.equal(L.fmtCompact(999), '999');
  assert.equal(L.fmtCoverage(0.874), '87%');
  assert.equal(L.fmtPct(12.345), '12.3%');
  assert.equal(L.fmtTime('2026-10-14T10:20:09Z'), '10:20:09');
  assert.equal(L.fmtTime('2026-10-14T10:20:09Z', true), '2026-10-14 10:20:09');
});

test('missing values sort last in both directions and ties keep their order', () => {
  const rows = [{ k: 3, n: 'a' }, { k: null, n: 'b' }, { k: 7, n: 'c' }, { k: 3, n: 'd' }];
  const names = (dir) => L.sortRows(rows, (r) => r.k, dir).map((r) => r.n).join('');
  assert.equal(names('desc'), 'cadb');
  assert.equal(names('asc'), 'adcb');
  assert.equal(L.sortRows([{ k: 'b' }, { k: 'A' }], (r) => r.k, 'asc')[0].k, 'A');
});

test('statement filters combine words, link and flag', () => {
  const rows = [
    { text: 'SELECT * FROM film', fingerprint: 'aaaa1111bbbb2222', link: 'both', stats: { flags: ['new'] } },
    { text: 'SELECT * FROM actor', fingerprint: 'cccc3333dddd4444', link: 'log', stats: null },
    { text: 'UPDATE film SET x = $1', fingerprint: 'eeee5555ffff6666', link: 'stats', stats: { flags: [] } },
  ];
  const texts = (q) => L.filterStatements(rows, q).map((r) => r.text);
  assert.equal(texts({ q: '' }).length, 3);
  assert.deepEqual(texts({ q: 'FILM select' }), ['SELECT * FROM film']);
  assert.deepEqual(texts({ q: 'cccc3333' }), ['SELECT * FROM actor']);
  assert.deepEqual(texts({ link: 'stats' }), ['UPDATE film SET x = $1']);
  assert.deepEqual(texts({ flag: 'new' }), ['SELECT * FROM film']);
  assert.deepEqual(texts({ q: 'nothing' }), []);
});

test('columns come from the statistics side when there is one', () => {
  const both = { link: 'both', text: 't', coverage: 0.5, log: { calls: 2, total_ms: 9, mean_ms: 4, p95_ms: 5, max_ms: 6, share_pct: 1, errors: 1 }, stats: { calls: 4, total_exec_ms: 400, mean_ms: 100, share_pct: 80 } };
  const logOnly = { link: 'log', text: 't', coverage: null, log: { calls: 2, total_ms: 9, mean_ms: 4, share_pct: 3 }, stats: null };
  assert.equal(L.column(both, 'total'), 400);
  assert.equal(L.column(logOnly, 'total'), 9);
  assert.equal(L.column(logOnly, 'stats_calls'), null);
  assert.equal(L.column(both, 'p95'), 5);
  assert.equal(L.column(both, 'share'), 80);
});

test('buckets are merged without losing counts', () => {
  const buckets = Array.from({ length: 10 }, (_, i) => ({ start: 's' + i, statements: i, errors: 1, connections: 0, lock_waits: 2 }));
  const small = L.downsample(buckets, 4);
  assert.equal(small.length, 4);
  assert.equal(small.reduce((s, b) => s + b.statements, 0), 45);
  assert.equal(small.reduce((s, b) => s + b.errors, 0), 10);
  assert.equal(small[0].start, 's0');
  assert.equal(small[0].span, 3);
  assert.equal(L.downsample(buckets, 50).length, 10);
  assert.deepEqual(L.downsample([], 5), []);
});

test('ranges are inclusive and tolerate reversed or oversized bounds', () => {
  const buckets = [{ e: 1 }, { e: 2 }, { e: 4 }, { e: 8 }];
  assert.equal(L.sumRange(buckets, 'e', null), 15);
  assert.equal(L.sumRange(buckets, 'e', { from: 1, to: 2 }), 6);
  assert.equal(L.sumRange(buckets, 'e', { from: 2, to: 1 }), 6);
  assert.equal(L.sumRange(buckets, 'e', { from: 2, to: 99 }), 12);
  assert.equal(L.sumRange([], 'e', null), 0);
});

test('axis maxima are round', () => {
  assert.equal(L.niceMax(0), 1);
  assert.equal(L.niceMax(7), 10);
  assert.equal(L.niceMax(1.2), 2);
  assert.equal(L.niceMax(230), 500);
});

test('the SQL tokenizer joins back to its input and tells the parts apart', () => {
  const sql = "SELECT a, \"Weird Col\", 1.5e3 FROM t WHERE x = $1 AND s = 'it''s' -- tail\n/* c */ OR y";
  const tokens = L.tokenizeSql(sql);
  assert.equal(tokens.map((t) => t.v).join(''), sql);
  const kinds = (type) => tokens.filter((t) => t.t === type).map((t) => t.v);
  assert.deepEqual(kinds('kw').map((s) => s.toLowerCase()), ['select', 'from', 'where', 'and', 'or']);
  assert.deepEqual(kinds('str'), ["'it''s'"]);
  assert.deepEqual(kinds('param'), ['$1']);
  assert.deepEqual(kinds('num'), ['1.5e3']);
  assert.deepEqual(kinds('comment'), ['-- tail', '/* c */']);
  assert.ok(kinds('id').includes('"Weird Col"'));
  assert.equal(L.tokenizeSql("'unterminated").map((t) => t.v).join(''), "'unterminated");
  assert.equal(L.tokenizeSql('').length, 0);
});

test('the URL hash round-trips and ignores junk', () => {
  const state = Object.assign({}, L.DEFAULT_STATE, { s: 'statements', q: 'film & actor', link: 'both', fp: 'abc', from: 3, to: 9, dir: 'asc' });
  const hash = L.buildHash(state);
  assert.deepEqual(L.parseHash(hash), state);
  assert.equal(L.buildHash(L.DEFAULT_STATE), '');
  assert.deepEqual(L.parseHash(''), L.DEFAULT_STATE);
  const junk = L.parseHash('#s=errors&evil=1&from=abc&to=-4&dir=sideways&q=%E0%A4%A');
  assert.equal(junk.s, 'errors');
  assert.equal(junk.from, null);
  assert.equal(junk.to, null);
  assert.equal(junk.dir, 'desc');
  assert.equal(junk.q, '');
  assert.equal(Object.prototype.hasOwnProperty.call(junk, 'evil'), false);
});

test('log positions stay inside the scale', () => {
  assert.equal(L.logPosition(1, 1, 100), 0);
  assert.equal(L.logPosition(100, 1, 100), 1);
  assert.ok(Math.abs(L.logPosition(10, 1, 100) - 0.5) < 1e-9);
  assert.equal(L.logPosition(0, 1, 100), 0);
  assert.equal(L.logPosition(1000, 1, 100), 1);
  assert.equal(L.logPosition(5, 5, 5), 0);
});

test('a selection is ordered and kept inside the buckets', () => {
  assert.deepEqual(L.clampRange(2, 5, 10), { from: 2, to: 5 });
  assert.deepEqual(L.clampRange(5, 2, 10), { from: 2, to: 5 });
  assert.deepEqual(L.clampRange(8, 99, 10), { from: 8, to: 9 });
  assert.equal(L.clampRange(500, 600, 10), null);
  assert.equal(L.clampRange(null, 3, 10), null);
  assert.equal(L.clampRange(0, 3, 0), null);
});

test('elapsed and upcoming times read like a person would say them', () => {
  assert.equal(L.fmtAgo(3), 'just now');
  assert.equal(L.fmtAgo(42), '42 s ago');
  assert.equal(L.fmtAgo(125), '2 min ago');
  assert.equal(L.fmtAgo(7300), '2 h ago');
  assert.equal(L.fmtAgo(-5), 'just now');
  assert.equal(L.fmtIn(0), 'any moment');
  assert.equal(L.fmtIn(-3), 'any moment');
  assert.equal(L.fmtIn(45), 'in 45 s');
  assert.equal(L.fmtIn(200), 'in 3 min');
  assert.equal(L.fmtIn(NaN), '—');
});

test('a newer generation is fetched; the same, or none built yet, is not', () => {
  assert.equal(L.shouldFetch({ generation: 3 }, 2), true);
  assert.equal(L.shouldFetch({ generation: 3 }, 3), false);
  assert.equal(L.shouldFetch({ generation: 0 }, 0), false);
  assert.equal(L.shouldFetch({ generation: 1 }, null), true);
  assert.equal(L.shouldFetch(null, 1), false);
  assert.equal(L.shouldFetch({}, 1), false);
});

test('a time selection follows its time when the window slides', () => {
  const bar = (m) => ({ start: new Date(Date.UTC(2026, 9, 14, 10, m)).toISOString() });
  const oldBars = [0, 5, 10, 15, 20, 25].map(bar);
  const newBars = [10, 15, 20, 25, 30, 35].map(bar);
  // 15:00-20:00 was bars 3..4; it is bars 1..2 after the window moved.
  assert.deepEqual(L.remapRange(oldBars, { from: 3, to: 4 }, newBars), { from: 1, to: 2 });
  // began before the new window: starts at the first bar
  assert.deepEqual(L.remapRange(oldBars, { from: 0, to: 3 }, newBars), { from: 0, to: 1 });
  // entirely before the new window: gone
  assert.equal(L.remapRange(oldBars, { from: 0, to: 1 }, newBars), null);
  assert.equal(L.remapRange(oldBars, null, newBars), null);
  assert.equal(L.remapRange(oldBars, { from: 3, to: 4 }, []), null);
});

test('times are read in the zone the report asks for', () => {
  const ts = '2026-10-14T12:30:15.123Z';
  assert.equal(L.fmtTime(ts, true), '2026-10-14 12:30:15');
  L.setZone('America/Sao_Paulo');
  try {
    assert.equal(L.fmtTime(ts, true), '2026-10-14 09:30:15');
    assert.equal(L.fmtTime(ts, false), '09:30:15');
    // The date moves with the clock.
    assert.equal(L.fmtTime('2026-10-14T02:00:00Z', true), '2026-10-13 23:00:00');
    L.setZone('Not/AZone');
    assert.equal(L.fmtTime(ts, false), '12:30:15', 'an unknown zone falls back to UTC');
  } finally {
    L.setZone(null);
  }
  assert.equal(L.fmtTime(ts, false), '12:30:15');
});

test('a new run id means a new report even with the same generation number', () => {
  assert.equal(L.shouldFetch({ generation: 1, run_id: 'b' }, { generation: 1, run_id: 'a' }), true);
  assert.equal(L.shouldFetch({ generation: 1, run_id: 'a' }, { generation: 1, run_id: 'a' }), false);
  assert.equal(L.shouldFetch({ generation: 2, run_id: 'a' }, { generation: 1, run_id: 'a' }), true);
  // Older pages and servers without run ids behave as before.
  assert.equal(L.shouldFetch({ generation: 2 }, 1), true);
  assert.equal(L.shouldFetch({ generation: 2 }, 2), false);
});
