// ==UserScript==
// @name         Magnet Link Handler for qBittorrent WebUI
// @namespace    http://tampermonkey.net/
// @version      2.1
// @author       down_to_earth
// @description  Intercept magnet links and open qBittorrent download dialog in a popup.
// @match        *://*/*
// @grant        GM_getValue
// @grant        GM_setValue
// @grant        GM_registerMenuCommand
// @license      MIT
// @downloadURL https://update.greasyfork.org/scripts/524742/Magnet%20Link%20Handler%20for%20qBittorrent%20WebUI.user.js
// @updateURL https://update.greasyfork.org/scripts/524742/Magnet%20Link%20Handler%20for%20qBittorrent%20WebUI.meta.js
// ==/UserScript==


(async function() {
    'use strict';

    async function sleep(ms) {
        return new Promise(resolve => setTimeout(resolve, ms));
    }

    function describeHttpError(status, body) {
        const messages = {
            400: 'Bad request — the server rejected the request. Check your settings and try again.',
            401: 'Unauthorized — you are not logged in. Please log in to the qBittorrent WebUI first.',
            403: 'Forbidden — you do not have permission to perform this action.',
            404: 'Not found — the API endpoint was not found. Check your qBittorrent WebUI URL.',
            409: 'This torrent already exists in qBittorrent. Remove it first if you want to re-add it.',
            415: 'Unsupported media type — the torrent file format was not recognized.',
            500: 'Internal server error — something went wrong on the qBittorrent server.',
        };
        return messages[status] || `Unexpected error (HTTP ${status}): ${body}`;
    }

    const defaultQbWebUIHost = 'https://qb.sduteam.com';
    let qbWebUIHost = GM_getValue('qbWebUIHost', defaultQbWebUIHost);

    GM_registerMenuCommand('Set qBittorrent WebUI URL', () => {
        const newUrl = prompt('Enter the qBittorrent WebUI URL:', qbWebUIHost);
        if (newUrl) {
            GM_setValue('qbWebUIHost', newUrl);
            qbWebUIHost = newUrl;
            alert(`qBittorrent WebUI URL set to: ${newUrl}`);
        }
    });

    const isDownloadPage = window.location.href.startsWith(qbWebUIHost)
        && window.location.pathname === '/download.html';

    if (isDownloadPage) {
        const urlParams = new URLSearchParams(window.location.search);
        if (!urlParams.get('source')) return;

        // Fallback: if the page still loads as "Unauthorized" (e.g. popup blocker
        // prevented the about:blank trick), reload once to clear the cross-site Referer.
        if (document.body?.textContent?.trim() === 'Unauthorized') {
            if (!sessionStorage.getItem('qb-unauthorized-reload')) {
                sessionStorage.setItem('qb-unauthorized-reload', '1');
                location.reload();
            }
            return;
        }
        sessionStorage.removeItem('qb-unauthorized-reload');

        // ── helpers ──────────────────────────────────────────────────────────

        function formatSize(bytes) {
            if (!bytes) return '0 B';
            const units = ['B', 'KB', 'MB', 'GB', 'TB'];
            let i = 0;
            while (bytes >= 1024 && i < units.length - 1) { bytes /= 1024; i++; }
            return bytes.toFixed(i > 1 ? 2 : 0) + ' ' + units[i];
        }

        function formatDate(timestamp) {
            if (!timestamp) return 'N/A';
            return new Date(timestamp * 1000).toLocaleDateString();
        }

        // Poll /api/v2/torrents/fetchMetadata until status 200 (202 = still fetching)
        async function pollMetadata(source) {
            for (let i = 0; i < 60; i++) {
                const fd = new FormData();
                fd.append('source', source);
                const resp = await fetch('/api/v2/torrents/fetchMetadata', { method: 'POST', body: fd });
                if (resp.status === 200) return resp.json();
                if (resp.status !== 202) throw new Error(`fetchMetadata returned ${resp.status}`);
                await sleep(1000);
            }
            throw new Error('Timed out waiting for torrent metadata (60 s)');
        }

        // ── options UI ───────────────────────────────────────────────────────

        const savedSettings = GM_getValue('qbSettings', {});
        function saveSettings(patch) {
            Object.assign(savedSettings, patch);
            GM_setValue('qbSettings', savedSettings);
        }

        function buildOptionsUI(metadata, categories, prefs, magnet) {
            const info = metadata?.info ?? {};
            const name = info.name || metadata?.name || 'Unknown Torrent';
            const totalSize = info.length || (info.files || []).reduce((s, f) => s + (f.length || 0), 0);

            let files = [];
            if (Array.isArray(info.files)) {
                files = info.files.map(f => ({
                    name: Array.isArray(f.path) ? f.path.join('/') : String(f.path ?? f.name ?? ''),
                    size: f.length ?? 0,
                }));
            } else if (info.length > 0) {
                files = [{ name, size: info.length }];
            }

            const catOptions = ['', ...Object.keys(categories || {})]
                .map(k => `<option value="${k}"${k === (savedSettings.category || '') ? ' selected' : ''}>${k || 'No category'}</option>`)
                .join('');

            const defaultSavePath = savedSettings.savepath || prefs.save_path || '';
            const esc = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

            // ── tree builder ─────────────────────────────────────────────────

            function buildFileTree(flatFiles) {
                const root = { name: '', isDir: true, children: [], size: 0 };
                flatFiles.forEach((f, idx) => {
                    const parts = f.name.split('/');
                    let cur = root;
                    for (let i = 0; i < parts.length - 1; i++) {
                        let dir = cur.children.find(c => c.isDir && c.name === parts[i]);
                        if (!dir) {
                            dir = { name: parts[i], isDir: true, children: [], size: 0 };
                            cur.children.push(dir);
                        }
                        cur = dir;
                    }
                    cur.children.push({ name: parts[parts.length - 1], isDir: false, size: f.size, idx });
                });
                (function calc(n) {
                    n.size = n.isDir ? n.children.reduce((s, c) => s + calc(c), 0) : n.size;
                    return n.size;
                })(root);
                return root;
            }

            let _gid = 0;
            function flattenTree(node, ancestors) {
                const rows = [];
                if (node.isDir && node.name) {
                    const gid = 'qbg' + (++_gid);
                    rows.push({ type: 'dir', name: node.name, size: node.size, gid, ancestors: [...ancestors] });
                    node.children.forEach(c => rows.push(...flattenTree(c, [...ancestors, gid])));
                } else if (!node.isDir) {
                    rows.push({ type: 'file', name: node.name, size: node.size, idx: node.idx, ancestors: [...ancestors] });
                } else {
                    node.children.forEach(c => rows.push(...flattenTree(c, [])));
                }
                return rows;
            }

            const treeRows = flattenTree(buildFileTree(files), []);
            // Start top-level directories expanded, deeper ones collapsed
            const collapsed = new Map(treeRows.filter(r => r.type === 'dir').map(r => [r.gid, r.ancestors.length > 0]));

            const prioOptions = `
                <option value="1" selected>Normal</option>
                <option value="6">High</option>
                <option value="7">Maximum</option>
                <option value="0">Do not download</option>`;

            const fileRows = treeRows.map(row => {
                const indent = row.ancestors.length * 20;
                const ancsAttr = row.ancestors.join(',');

                if (row.type === 'dir') {
                    return `<tr class="qb-tree-row" data-ancestors="${ancsAttr}" data-gid="${row.gid}">
                        <td class="qb-td-cb"><input type="checkbox" class="qb-dir-cb" data-gid="${row.gid}" checked></td>
                        <td class="qb-td-name">
                            <span style="padding-left:${indent}px" class="qb-name-cell">
                                <button class="qb-toggle" data-gid="${row.gid}">▶</button>
                                <span class="qb-folder-icon">📁</span>
                                ${esc(row.name)}
                            </span>
                        </td>
                        <td class="qb-td-size">${formatSize(row.size)}</td>
                        <td class="qb-td-prio"><select class="qb-dir-priority" data-gid="${row.gid}">${prioOptions}</select></td>
                    </tr>`;
                } else {
                    return `<tr class="qb-tree-row" data-ancestors="${ancsAttr}">
                        <td class="qb-td-cb"><input type="checkbox" class="qb-file-cb" data-idx="${row.idx}" checked></td>
                        <td class="qb-td-name">
                            <span style="padding-left:${indent + (treeRows.some(r => r.type === 'dir') ? 22 : 0)}px" class="qb-name-cell qb-name-file">
                                ${esc(row.name)}
                            </span>
                        </td>
                        <td class="qb-td-size">${formatSize(row.size)}</td>
                        <td class="qb-td-prio"><select class="qb-file-priority" data-idx="${row.idx}">${prioOptions}</select></td>
                    </tr>`;
                }
            }).join('');

            // ── inject styles ────────────────────────────────────────────────

            document.head.insertAdjacentHTML('beforeend', `<style>
                html, body { margin:0 !important; padding:0 !important; background:#1e1e1e !important; color:#ddd !important;
                    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif !important; font-size:13px !important; }
                .qb-panel { display:flex !important; height:100vh; overflow:hidden; background:#1e1e1e; color:#ddd; }
                .qb-left { width:340px; min-width:260px; flex-shrink:0; overflow-y:auto; padding:12px;
                    border-right:1px solid #3a3a3a; box-sizing:border-box; background:#252525; }
                .qb-right { flex:1; display:flex; flex-direction:column; overflow:hidden; background:#1e1e1e; }
                .qb-right-header { padding:8px 12px; border-bottom:1px solid #3a3a3a; background:#252525; }
                .qb-file-table-wrap { flex:1; overflow-y:auto; }
                .qb-section { margin-bottom:12px; }
                .qb-label { font-size:11px; color:#999; margin-bottom:2px; display:block; }
                .qb-input { width:100%; box-sizing:border-box; padding:4px 6px; border:1px solid #4a4a4a;
                    border-radius:3px; font-size:12px; background:#333; color:#e0e0e0; }
                .qb-input::placeholder { color:#666; }
                .qb-select { width:100%; box-sizing:border-box; padding:4px 6px; border:1px solid #4a4a4a;
                    border-radius:3px; font-size:12px; background:#333; color:#e0e0e0; }
                .qb-checkbox-row { display:flex; align-items:center; gap:6px; margin:5px 0; font-size:12px;
                    cursor:pointer; color:#ddd; }
                .qb-checkbox-row input { cursor:pointer; accent-color:#1976d2; }
                .qb-info-table { width:100%; font-size:11px; border-collapse:collapse; }
                .qb-info-table td { padding:2px 0; vertical-align:top; color:#ddd; }
                .qb-info-table td:first-child { color:#888; white-space:nowrap; padding-right:8px; }
                .qb-bottom { display:flex; gap:8px; padding:10px 12px; border-top:1px solid #3a3a3a;
                    background:#252525; justify-content:flex-end; }
                .qb-btn { padding:6px 18px; border-radius:4px; cursor:pointer; font-size:13px; border:1px solid #555; }
                .qb-btn-primary { background:#1976d2; color:#fff; border-color:#1565c0; }
                .qb-btn-primary:hover { background:#1565c0; }
                .qb-btn-primary:disabled { background:#555; border-color:#555; cursor:not-allowed; }
                .qb-btn-secondary { background:#3a3a3a; color:#ddd; }
                .qb-btn-secondary:hover { background:#444; }
                .qb-file-table { width:100%; border-collapse:collapse; table-layout:fixed; }
                .qb-file-table th { background:#2d2d2d; border-bottom:1px solid #3a3a3a; padding:5px 4px;
                    font-size:11px; text-align:left; position:sticky; top:0; color:#bbb; z-index:1; }
                .qb-td-cb { width:28px; padding:2px 4px; text-align:center; vertical-align:middle; }
                .qb-td-cb input { accent-color:#1976d2; cursor:pointer; }
                .qb-td-name { padding:2px 4px; vertical-align:middle; overflow:hidden; }
                .qb-td-size { padding:2px 6px; text-align:right; white-space:nowrap; font-size:12px; color:#888; width:80px; vertical-align:middle; }
                .qb-td-prio { padding:2px 4px; width:120px; vertical-align:middle; }
                .qb-file-table td { border-bottom:1px solid #2a2a2a; color:#ddd; }
                .qb-tree-row:hover td { background:#2a3a4a; }
                .qb-name-cell { display:flex; align-items:center; gap:4px; font-size:12px; white-space:nowrap;
                    overflow:hidden; text-overflow:ellipsis; }
                .qb-name-file { color:#ccc; }
                .qb-folder-icon { font-size:13px; flex-shrink:0; }
                .qb-toggle { background:none; border:none; cursor:pointer; color:#888; font-size:10px;
                    padding:0 3px; line-height:1; flex-shrink:0; transition:transform 0.1s; }
                .qb-toggle:hover { color:#ddd; }
                .qb-file-priority, .qb-dir-priority { background:#333; color:#e0e0e0; border:1px solid #4a4a4a;
                    border-radius:2px; font-size:11px; padding:2px; width:100%; }
                .qb-divider { font-size:11px; font-weight:bold; color:#bbb; border-bottom:1px solid #3a3a3a;
                    padding-bottom:3px; margin-bottom:8px; }
                .qb-indent { margin-left:12px; }
                .qb-rate-row { display:flex; gap:6px; align-items:center; }
                .qb-rate-row .qb-input { flex:1; }
                .qb-rate-row span { font-size:11px; color:#888; white-space:nowrap; }
            </style>`);

            // ── build DOM ────────────────────────────────────────────────────

            const s = savedSettings;
            const sel = (val, cur) => val === cur ? ' selected' : '';
            const chk = (key) => s[key] ? ' checked' : '';
            const incompletePath = s.incompletePath || prefs.temp_path || '';
            const incompleteOn = s.incompleteToggle || false;

            document.body.innerHTML = `
<div class="qb-panel">
  <div class="qb-left">
    <div class="qb-section">
        <div class="qb-divider">Torrent Management</div>
        <label class="qb-label">Torrent management mode</label>
        <select id="qb-mode" class="qb-select">
            <option value=""${sel(s.mode || '', '')}>Default (Use default mode)</option>
            <option value="manual"${sel(s.mode, 'manual')}>Manual</option>
            <option value="automatic"${sel(s.mode, 'automatic')}>Automatic</option>
        </select>
    </div>

    <div class="qb-section">
        <div class="qb-divider">Save Location</div>
        <label class="qb-label">Save files to</label>
        <input id="qb-savepath" type="text" class="qb-input" value="${esc(defaultSavePath)}">
        <label class="qb-checkbox-row" style="margin-top:6px;">
            <input type="checkbox" id="qb-incomplete-toggle"${chk('incompleteToggle')}> Use another path for incomplete torrent
        </label>
        <div id="qb-incomplete-wrap" style="display:${incompleteOn ? 'block' : 'none'};margin-top:4px;">
            <label class="qb-label">Incomplete files path</label>
            <input id="qb-incomplete-path" type="text" class="qb-input" value="${esc(incompletePath)}">
        </div>
        <label class="qb-label" style="margin-top:8px;">Rename torrent</label>
        <input id="qb-rename" type="text" class="qb-input" value="${esc(name)}">
    </div>

    <div class="qb-section">
        <div class="qb-divider">Category &amp; Tags</div>
        <label class="qb-label">Category</label>
        <select id="qb-category" class="qb-select">${catOptions}</select>
        <label class="qb-label" style="margin-top:6px;">Tags <span style="color:#666;font-size:10px;">(comma-separated)</span></label>
        <input id="qb-tags" type="text" class="qb-input" placeholder="e.g. movies, 4k" value="${esc(s.tags || '')}">
    </div>

    <div class="qb-section">
        <div class="qb-divider">Options</div>
        <label class="qb-checkbox-row">
            <input type="checkbox" id="qb-paused"${chk('startTorrent')}> Start torrent
        </label>
        <label class="qb-label qb-indent" style="margin-top:4px;">Stop condition</label>
        <select id="qb-stop-condition" class="qb-select qb-indent">
            <option value=""${sel(s.stopCondition || '', '')}>None</option>
            <option value="MetadataReceived"${sel(s.stopCondition, 'MetadataReceived')}>Metadata received</option>
            <option value="FilesChecked"${sel(s.stopCondition, 'FilesChecked')}>Files checked</option>
        </select>
        <label class="qb-checkbox-row" style="margin-top:6px;">
            <input type="checkbox" id="qb-top-queue"${chk('topQueue')}> Add to top of queue
        </label>
        <label class="qb-checkbox-row">
            <input type="checkbox" id="qb-skip-check"${chk('skipCheck')}> Skip hash check
        </label>
        <label class="qb-checkbox-row">
            <input type="checkbox" id="qb-sequential"${chk('sequential')}> Download in sequential order
        </label>
        <label class="qb-checkbox-row">
            <input type="checkbox" id="qb-first-last"${chk('firstLast')}> Download first and last pieces first
        </label>
        <label class="qb-checkbox-row" style="margin-top:6px;">
            <input type="checkbox" id="qb-open-app"${chk('openApp')}> Open qBittorrent after adding
        </label>
    </div>

    <div class="qb-section">
        <div class="qb-divider">Content Layout</div>
        <select id="qb-content-layout" class="qb-select">
            <option value=""${sel(s.contentLayout || '', '')}>Default</option>
            <option value="Original"${sel(s.contentLayout, 'Original')}>Original</option>
            <option value="Subfolder"${sel(s.contentLayout, 'Subfolder')}>Create subfolder</option>
            <option value="NoSubfolder"${sel(s.contentLayout, 'NoSubfolder')}>Don't create subfolder</option>
        </select>
    </div>

    <div class="qb-section">
        <div class="qb-divider">Speed Limits</div>
        <label class="qb-label">Download limit</label>
        <div class="qb-rate-row">
            <input id="qb-dl-limit" type="number" class="qb-input" placeholder="0 = unlimited" min="0" value="${s.dlLimit || 0}">
            <span>KiB/s</span>
        </div>
        <label class="qb-label" style="margin-top:6px;">Upload limit</label>
        <div class="qb-rate-row">
            <input id="qb-ul-limit" type="number" class="qb-input" placeholder="0 = unlimited" min="0" value="${s.ulLimit || 0}">
            <span>KiB/s</span>
        </div>
    </div>

    <div class="qb-section">
        <div class="qb-divider">Torrent Information</div>
        <table class="qb-info-table">
            <tr><td>Size:</td><td>${formatSize(totalSize)}</td></tr>
            <tr><td>Info hash v1:</td><td style="word-break:break-all;">${esc(info['sha1-hash'] || info.hash || metadata?.hash || 'N/A')}</td></tr>
            <tr><td>Comment:</td><td style="word-break:break-all;">${esc(info.comment || metadata?.comment || 'N/A')}</td></tr>
            <tr><td>Date:</td><td>${formatDate(metadata?.['creation date'])}</td></tr>
        </table>
    </div>
  </div>

  <div class="qb-right">
    <div class="qb-right-header">
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:6px;">
            <span style="font-weight:bold;font-size:13px;">Files (${files.length})</span>
            <label style="display:flex;align-items:center;gap:4px;font-size:11px;cursor:pointer;color:#ddd;">
                <input type="checkbox" id="qb-selectall" checked style="accent-color:#1976d2;"> Select all
            </label>
        </div>
        <input id="qb-filter" type="text" class="qb-input" placeholder="Filter files..." style="font-size:12px;">
    </div>
    <div class="qb-file-table-wrap">
        <table class="qb-file-table">
            <thead>
                <tr>
                    <th class="qb-td-cb"></th>
                    <th class="qb-td-name">Name</th>
                    <th class="qb-td-size" style="text-align:right;">Total Size</th>
                    <th class="qb-td-prio">Download Priority</th>
                </tr>
            </thead>
            <tbody id="qb-file-tbody">${fileRows}</tbody>
        </table>
    </div>
    <div class="qb-bottom">
        <button id="qb-cancel" class="qb-btn qb-btn-secondary">Cancel</button>
        <button id="qb-submit" class="qb-btn qb-btn-primary">Add Torrent</button>
    </div>
  </div>
</div>`;

            // ── tree collapse / expand ────────────────────────────────────────

            function updateVisibility() {
                document.querySelectorAll('.qb-tree-row').forEach(row => {
                    const ancs = row.dataset.ancestors ? row.dataset.ancestors.split(',').filter(Boolean) : [];
                    row.style.display = ancs.some(a => collapsed.get(a)) ? 'none' : '';
                });
                document.querySelectorAll('.qb-toggle').forEach(btn => {
                    btn.textContent = collapsed.get(btn.dataset.gid) ? '▶' : '▼';
                });
            }
            updateVisibility();

            document.getElementById('qb-file-tbody').addEventListener('click', e => {
                const btn = e.target.closest('.qb-toggle');
                if (!btn) return;
                collapsed.set(btn.dataset.gid, !collapsed.get(btn.dataset.gid));
                updateVisibility();
            });

            // ── checkbox & priority sync ──────────────────────────────────────

            function getDescendantFileCbs(gid) {
                return [...document.querySelectorAll('.qb-file-cb')].filter(cb => {
                    const row = cb.closest('.qb-tree-row');
                    return row?.dataset.ancestors?.split(',').includes(gid);
                });
            }

            function updateDirStates() {
                document.querySelectorAll('.qb-dir-cb').forEach(dirCb => {
                    const desc = getDescendantFileCbs(dirCb.dataset.gid);
                    if (!desc.length) return;
                    const n = desc.filter(c => c.checked).length;
                    dirCb.checked = n > 0;
                    dirCb.indeterminate = n > 0 && n < desc.length;
                });
            }

            function updateSelectAll() {
                const all = [...document.querySelectorAll('.qb-file-cb')];
                const n = all.filter(c => c.checked).length;
                const sa = document.getElementById('qb-selectall');
                if (sa) { sa.checked = n === all.length; sa.indeterminate = n > 0 && n < all.length; }
            }

            document.getElementById('qb-file-tbody').addEventListener('change', e => {
                const t = e.target;

                if (t.matches('.qb-dir-cb')) {
                    getDescendantFileCbs(t.dataset.gid).forEach(cb => {
                        cb.checked = t.checked;
                        const sel = document.querySelector(`.qb-file-priority[data-idx="${cb.dataset.idx}"]`);
                        if (sel) sel.value = t.checked ? (sel.value === '0' ? '1' : sel.value) : '0';
                    });
                    updateDirStates();
                    updateSelectAll();
                }

                if (t.matches('.qb-file-cb')) {
                    const sel = document.querySelector(`.qb-file-priority[data-idx="${t.dataset.idx}"]`);
                    if (sel) { if (!t.checked) sel.value = '0'; else if (sel.value === '0') sel.value = '1'; }
                    updateDirStates();
                    updateSelectAll();
                }

                if (t.matches('.qb-dir-priority')) {
                    getDescendantFileCbs(t.dataset.gid).forEach(cb => {
                        const sel = document.querySelector(`.qb-file-priority[data-idx="${cb.dataset.idx}"]`);
                        if (sel) sel.value = t.value;
                        cb.checked = t.value !== '0';
                    });
                    updateDirStates();
                    updateSelectAll();
                }

                if (t.matches('.qb-file-priority')) {
                    const cb = document.querySelector(`.qb-file-cb[data-idx="${t.dataset.idx}"]`);
                    if (cb) cb.checked = t.value !== '0';
                    updateDirStates();
                    updateSelectAll();
                }
            });

            // ── select all ───────────────────────────────────────────────────

            document.getElementById('qb-selectall').addEventListener('change', e => {
                document.querySelectorAll('.qb-file-cb').forEach(cb => {
                    cb.checked = e.target.checked;
                    const sel = document.querySelector(`.qb-file-priority[data-idx="${cb.dataset.idx}"]`);
                    if (sel) sel.value = e.target.checked ? '1' : '0';
                });
                updateDirStates();
            });

            // ── file filter ──────────────────────────────────────────────────

            document.getElementById('qb-filter').addEventListener('input', e => {
                const q = e.target.value.toLowerCase();
                document.querySelectorAll('.qb-tree-row').forEach(row => {
                    if (!q) {
                        // Restore collapse state
                        const ancs = row.dataset.ancestors ? row.dataset.ancestors.split(',').filter(Boolean) : [];
                        row.style.display = ancs.some(a => collapsed.get(a)) ? 'none' : '';
                    } else {
                        const cellName = row.querySelector('.qb-name-cell')?.textContent?.toLowerCase() || '';
                        row.style.display = cellName.includes(q) ? '' : 'none';
                    }
                });
            });

            // ── incomplete path toggle ────────────────────────────────────────

            document.getElementById('qb-incomplete-toggle').addEventListener('change', e => {
                document.getElementById('qb-incomplete-wrap').style.display = e.target.checked ? 'block' : 'none';
                saveSettings({ incompleteToggle: e.target.checked });
            });

            // ── persist settings on change ───────────────────────────────────

            const settingsMap = {
                'qb-mode':           { key: 'mode',           type: 'value' },
                'qb-savepath':       { key: 'savepath',       type: 'value' },
                'qb-incomplete-path':{ key: 'incompletePath', type: 'value' },
                'qb-category':       { key: 'category',       type: 'value' },
                'qb-tags':           { key: 'tags',           type: 'value' },
                'qb-paused':         { key: 'startTorrent',   type: 'checked' },
                'qb-stop-condition': { key: 'stopCondition',  type: 'value' },
                'qb-top-queue':      { key: 'topQueue',       type: 'checked' },
                'qb-skip-check':     { key: 'skipCheck',      type: 'checked' },
                'qb-sequential':     { key: 'sequential',     type: 'checked' },
                'qb-first-last':     { key: 'firstLast',      type: 'checked' },
                'qb-open-app':       { key: 'openApp',        type: 'checked' },
                'qb-content-layout': { key: 'contentLayout',  type: 'value' },
                'qb-dl-limit':       { key: 'dlLimit',        type: 'value' },
                'qb-ul-limit':       { key: 'ulLimit',        type: 'value' },
            };
            for (const [id, cfg] of Object.entries(settingsMap)) {
                const el = document.getElementById(id);
                if (!el) continue;
                el.addEventListener('change', () => {
                    saveSettings({ [cfg.key]: cfg.type === 'checked' ? el.checked : el.value });
                });
                if (cfg.type === 'value' && el.tagName === 'INPUT') {
                    el.addEventListener('input', () => {
                        saveSettings({ [cfg.key]: el.value });
                    });
                }
            }

            // ── cancel / submit ───────────────────────────────────────────────

            document.getElementById('qb-cancel').addEventListener('click', () => window.close());

            document.getElementById('qb-submit').addEventListener('click', async () => {
                const btn = document.getElementById('qb-submit');
                btn.disabled = true;
                btn.textContent = 'Adding…';

                const fd = new FormData();
                fd.append('urls', magnet);
                fd.append('savepath', document.getElementById('qb-savepath').value.trim());

                const rename = document.getElementById('qb-rename').value.trim();
                if (rename && rename !== name) fd.append('rename', rename);

                const category = document.getElementById('qb-category').value;
                if (category) fd.append('category', category);

                const tags = document.getElementById('qb-tags').value.trim();
                if (tags) fd.append('tags', tags);

                const mode = document.getElementById('qb-mode').value;
                if (mode) fd.append('useAutoTMM', mode === 'automatic' ? 'true' : 'false');

                if (!document.getElementById('qb-paused').checked) {
                    fd.append('stopped', 'true');
                    fd.append('paused', 'true');
                }

                const stopCond = document.getElementById('qb-stop-condition').value;
                if (stopCond) fd.append('stopCondition', stopCond);

                if (document.getElementById('qb-top-queue').checked) fd.append('addToTopOfQueue', 'true');
                if (document.getElementById('qb-skip-check').checked) fd.append('skip_checking', 'true');
                if (document.getElementById('qb-sequential').checked) fd.append('sequentialDownload', 'true');
                if (document.getElementById('qb-first-last').checked) fd.append('firstLastPiecePrio', 'true');

                const contentLayout = document.getElementById('qb-content-layout').value;
                if (contentLayout) fd.append('contentLayout', contentLayout);

                const dlLimit = parseInt(document.getElementById('qb-dl-limit').value) || 0;
                if (dlLimit > 0) fd.append('dlLimit', String(dlLimit * 1024));

                const ulLimit = parseInt(document.getElementById('qb-ul-limit').value) || 0;
                if (ulLimit > 0) fd.append('upLimit', String(ulLimit * 1024));

                const incompletePath = document.getElementById('qb-incomplete-toggle').checked
                    ? document.getElementById('qb-incomplete-path').value.trim()
                    : '';
                if (incompletePath) fd.append('tempPathEnabled', 'true');

                // Collect file priorities sorted by index
                const prioritySelects = [...document.querySelectorAll('.qb-file-priority')]
                    .sort((a, b) => parseInt(a.dataset.idx) - parseInt(b.dataset.idx));
                const priorities = prioritySelects.map(s => parseInt(s.value));
                const hash = metadata?.hash;

                try {
                    const resp = await fetch('/api/v2/torrents/add', { method: 'POST', body: fd });
                    const text = await resp.text();

                    let ok = text === 'Ok.';
                    if (!ok) {
                        try { ok = JSON.parse(text).failure_count === 0; } catch {}
                    }

                    if (!ok) {
                        alert(`Failed to add torrent:\n${describeHttpError(resp.status, text)}`);
                        btn.disabled = false;
                        btn.textContent = 'Add Torrent';
                        return;
                    }

                    // Set file priorities after adding
                    if (hash && priorities.length > 0) {
                        const byPriority = {};
                        priorities.forEach((p, i) => {
                            (byPriority[p] ??= []).push(i);
                        });
                        for (const [prio, indices] of Object.entries(byPriority)) {
                            if (prio === '1') continue;
                            const prioFd = new FormData();
                            prioFd.append('hash', hash);
                            prioFd.append('id', indices.join('|'));
                            prioFd.append('priority', prio);
                            await fetch('/api/v2/torrents/filePrio', { method: 'POST', body: prioFd });
                        }
                    }

                    if (document.getElementById('qb-open-app').checked) {
                        window.open(qbWebUIHost, '_blank');
                    }
                    window.close();
                } catch (err) {
                    alert(`Error: ${err.message}`);
                    btn.disabled = false;
                    btn.textContent = 'Add Torrent';
                }
            });
        }

        // ── intercept Download button ────────────────────────────────────────

        document.addEventListener('click', async (e) => {
            if (e.target.id !== 'submitButton') return;
            e.preventDefault();
            e.stopImmediatePropagation();

            const magnet = document.getElementById('urls')?.value?.trim();
            if (!magnet) return;

            document.body.innerHTML =
                '<div style="padding:20px;font-family:sans-serif;color:#aaa;background:#1e1e1e;height:100vh;box-sizing:border-box;">Fetching torrent metadata…</div>';

            try {
                const [metadata, catResp, prefsResp] = await Promise.all([
                    pollMetadata(magnet),
                    fetch('/api/v2/torrents/categories').catch(() => null),
                    fetch('/api/v2/app/preferences').catch(() => null),
                ]);
                const categories = catResp?.ok ? await catResp.json().catch(() => ({})) : {};
                const prefs      = prefsResp?.ok ? await prefsResp.json().catch(() => ({})) : {};

                buildOptionsUI(metadata, categories, prefs, magnet);
            } catch (err) {
                document.body.innerHTML =
                    `<div style="padding:20px;font-family:sans-serif;color:#e55;background:#1e1e1e;height:100vh;box-sizing:border-box;">${err.message}</div>`;
            }
        }, true);

        // Auto-submit for Ctrl+click / middle-click
        if (urlParams.get('autoDownload') === 'true') {
            while (!document.querySelector('#urls')?.value) {
                await sleep(10);
            }
            document.querySelector('#submitButton')?.click();
        }

    } else if (!window.location.href.startsWith(qbWebUIHost)) {
        // Any non-qBittorrent page: intercept magnet link clicks
        function handleMagnetLinkClick(event) {
            if ((event.type === 'click' && event.button !== 0) ||
                (event.type === 'mouseup' && event.button !== 1)) {
                return;
            }

            let target = event.target;
            while (target && target.tagName !== 'A') {
                target = target.parentElement;
            }

            if (target?.href.startsWith('magnet:')) {
                event.preventDefault();

                const encodedMagnetLink = encodeURIComponent(target.href);
                const sourceHostname = window.location.hostname;
                const autoDownload = event.ctrlKey || event.button === 1;

                const popupUrl = `${qbWebUIHost}/download.html?urls=${encodedMagnetLink}&source=${sourceHostname}&autoDownload=${autoDownload}`;

                const popupWidth = 1150;
                const popupHeight = 750;
                const left = (window.screen.width - popupWidth) / 2;
                const top = (window.screen.height - popupHeight) / 2;

                let popupName = 'qBittorrentAddMagnet';
                if (autoDownload) popupName += crypto.randomUUID();

                const popup = window.open('about:blank', popupName,
                    `width=${popupWidth},height=${popupHeight},left=${left},top=${top}`);
                if (popup) popup.location.href = popupUrl;
            }
        }

        document.addEventListener('click', handleMagnetLinkClick, true);
        document.addEventListener('mouseup', handleMagnetLinkClick, true);
    }
})();
