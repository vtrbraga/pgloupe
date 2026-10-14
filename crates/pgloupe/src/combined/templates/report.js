(function () {
  var table = document.getElementById('statements-table');
  if (!table) { return; }
  var body = table.tBodies[0];
  function value(row, index) {
    var cell = row.cells[index];
    var v = cell.getAttribute('data-v');
    if (v === null) { return cell.textContent.trim().toLowerCase(); }
    var n = parseFloat(v);
    return isNaN(n) ? -Infinity : n;
  }
  Array.prototype.forEach.call(table.tHead.rows[0].cells, function (th, index) {
    if (!th.hasAttribute('data-sort')) { return; }
    th.addEventListener('click', function () {
      var dir = th.getAttribute('data-dir') === 'desc' ? 'asc' : 'desc';
      Array.prototype.forEach.call(th.parentNode.cells, function (c) { c.removeAttribute('data-dir'); });
      th.setAttribute('data-dir', dir);
      var rows = Array.prototype.slice.call(body.rows);
      rows.sort(function (a, b) {
        var x = value(a, index), y = value(b, index);
        var c = x < y ? -1 : x > y ? 1 : 0;
        return dir === 'desc' ? -c : c;
      });
      rows.forEach(function (r) { body.appendChild(r); });
    });
  });
  var filter = document.getElementById('filter');
  if (filter) {
    filter.addEventListener('input', function () {
      var q = filter.value.toLowerCase();
      Array.prototype.forEach.call(body.rows, function (r) {
        r.hidden = q !== '' && r.getAttribute('data-text').indexOf(q) === -1;
      });
    });
  }
})();
