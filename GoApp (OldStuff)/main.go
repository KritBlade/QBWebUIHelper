package main

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/jchv/go-webview2"
)

const (
	appName    = "QBWebUIHelper"
	appVersion = "1.0.0"
	ipcAddr    = "127.0.0.1:47683"
)

type Config struct {
	WebUIURL string `json:"webui_url"`
	Width    int    `json:"width"`
	Height   int    `json:"height"`
}

type PendingAction struct {
	Type     string `json:"type"`
	URL      string `json:"url,omitempty"`
	Filename string `json:"filename,omitempty"`
	Data     string `json:"data,omitempty"`
}

var (
	wv        webview2.WebView
	pendingMu sync.Mutex
	pending   *PendingAction
)

func main() {
	if len(os.Args) > 1 {
		switch os.Args[1] {
		case "--register":
			registerAssociations()
			return
		case "--unregister":
			unregisterAssociations()
			return
		case "--help", "-h":
			fmt.Println(`QBWebUIHelper - qBittorrent WebUI Desktop Wrapper

Usage:
  qbhelper                     Open qBittorrent WebUI
  qbhelper <file.torrent>      Open and add torrent
  qbhelper <magnet:...>        Open and add magnet link
  qbhelper --register          Register as .torrent/magnet handler
  qbhelper --unregister        Remove file associations`)
			return
		}
	}

	arg := ""
	if len(os.Args) > 1 {
		arg = os.Args[1]
	}

	if arg != "" {
		allowForeground()
		if trySend(arg) {
			return
		}
	}

	ln, err := net.Listen("tcp", ipcAddr)
	if err != nil {
		time.Sleep(300 * time.Millisecond)
		if arg != "" && trySend(arg) {
			return
		}
	} else {
		go acceptIPC(ln)
	}

	if arg != "" {
		pending = buildAction(arg)
	}

	startApp(loadConfig())
}

func buildAction(arg string) *PendingAction {
	if strings.HasPrefix(arg, "magnet:") {
		return &PendingAction{Type: "magnet", URL: arg}
	}
	data, err := os.ReadFile(arg)
	if err != nil {
		return nil
	}
	return &PendingAction{
		Type:     "torrent",
		Filename: filepath.Base(arg),
		Data:     base64.StdEncoding.EncodeToString(data),
	}
}

func dataDir() string {
	dir := filepath.Join(os.Getenv("LOCALAPPDATA"), appName)
	os.MkdirAll(dir, 0755)
	return dir
}

func loadConfig() *Config {
	cfg := &Config{
		WebUIURL: "http://10.0.1.249:9865",
		Width:    1600,
		Height:   900,
	}
	data, err := os.ReadFile(filepath.Join(dataDir(), "config.json"))
	if err == nil {
		json.Unmarshal(data, cfg)
	}
	if cfg.Width < 400 {
		cfg.Width = 1600
	}
	if cfg.Height < 300 {
		cfg.Height = 900
	}
	return cfg
}

func saveConfig(cfg *Config) {
	data, _ := json.MarshalIndent(cfg, "", "  ")
	os.WriteFile(filepath.Join(dataDir(), "config.json"), data, 0644)
}

// --- IPC for single-instance ---

func trySend(arg string) bool {
	conn, err := net.DialTimeout("tcp", ipcAddr, 500*time.Millisecond)
	if err != nil {
		return false
	}
	defer conn.Close()
	conn.Write([]byte(arg))
	return true
}

func acceptIPC(ln net.Listener) {
	defer ln.Close()
	for {
		conn, err := ln.Accept()
		if err != nil {
			continue
		}
		go func() {
			defer conn.Close()
			data, _ := io.ReadAll(io.LimitReader(conn, 1<<20))
			if len(data) > 0 {
				onNewArg(string(data))
			}
		}()
	}
}

func onNewArg(arg string) {
	if wv == nil {
		return
	}
	action := buildAction(arg)
	if action == nil {
		return
	}
	actionJSON, _ := json.Marshal(action)
	wv.Dispatch(func() {
		bringToFront()
		wv.Eval(fmt.Sprintf("window.__qbHelper_handle(%s)", string(actionJSON)))
	})
}

// --- WebView app ---

func startApp(cfg *Config) {
	wv = webview2.NewWithOptions(webview2.WebViewOptions{
		Debug:     true,
		AutoFocus: true,
		DataPath:  filepath.Join(dataDir(), "webview2"),
		WindowOptions: webview2.WindowOptions{
			Title:  "qBittorrent WebUI",
			Width:  uint(cfg.Width),
			Height: uint(cfg.Height),
			Center: true,
		},
	})
	if wv == nil {
		showErrorDialog("Failed to create WebView2 window.\nMake sure Edge WebView2 Runtime is installed.")
		os.Exit(1)
	}
	defer wv.Destroy()

	wv.Bind("__qbHelper_getPending", func() *PendingAction {
		pendingMu.Lock()
		defer pendingMu.Unlock()
		a := pending
		pending = nil
		return a
	})

	wv.Bind("__qbHelper_saveSize", func(w, h int) {
		if w > 0 && h > 0 {
			cfg.Width = w
			cfg.Height = h
			saveConfig(cfg)
		}
	})

	wv.Bind("__qbHelper_register", func() {
		registerAssociations()
	})

	wv.Bind("__qbHelper_unregister", func() {
		unregisterAssociations()
	})

	wv.Bind("__qbHelper_isRegistered", func() bool {
		return isRegistered()
	})

	wv.Bind("__qbHelper_openDefaultApps", func() {
		openDefaultApps()
	})

	wv.Bind("__qbHelper_getURL", func() string {
		return cfg.WebUIURL
	})

	wv.Bind("__qbHelper_setURL", func(url string) {
		cfg.WebUIURL = url
		saveConfig(cfg)
	})

	wv.Init(helperJS())
	wv.Init(settingsJS())
	wv.Navigate(cfg.WebUIURL)
	wv.Run()
}

func helperJS() string {
	return `
window.__qbHelper_handle = function(action) {
    if (action.type === 'magnet') {
        __qbHelper_addMagnet(action.url);
    } else if (action.type === 'torrent') {
        __qbHelper_addTorrent(action.filename, action.data);
    }
};

function __qbHelper_addMagnet(url) {
    function attempt(n) {
        if (n <= 0) return;
        if (typeof showDownloadPage === 'function') {
            showDownloadPage([url]);
        } else {
            setTimeout(function() { attempt(n - 1); }, 300);
        }
    }
    attempt(30);
}

function __qbHelper_addTorrent(filename, b64data) {
    function attempt(n) {
        if (n <= 0) return;
        var fn = window.qBittorrent && window.qBittorrent.Client && window.qBittorrent.Client.uploadTorrentFiles;
        if (typeof fn === 'function') {
            var binary = atob(b64data);
            var bytes = new Uint8Array(binary.length);
            for (var i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
            var file = new File([bytes], filename, {type: 'application/x-bittorrent'});
            fn([file]);
        } else {
            setTimeout(function() { attempt(n - 1); }, 300);
        }
    }
    attempt(30);
}

var __qbSizeTimer;
window.addEventListener('resize', function() {
    clearTimeout(__qbSizeTimer);
    __qbSizeTimer = setTimeout(function() {
        var dpr = window.devicePixelRatio || 1;
        __qbHelper_saveSize(
            Math.round(window.outerWidth * dpr),
            Math.round(window.outerHeight * dpr)
        );
    }, 500);
});

__qbHelper_getPending().then(function(action) {
    if (!action) return;
    function waitReady(n) {
        if (n <= 0) return;
        var ready = (action.type === 'magnet')
            ? (typeof showDownloadPage === 'function')
            : (window.qBittorrent && window.qBittorrent.Client && typeof window.qBittorrent.Client.uploadTorrentFiles === 'function');
        if (ready) {
            window.__qbHelper_handle(action);
        } else {
            setTimeout(function() { waitReady(n - 1); }, 300);
        }
    }
    if (document.readyState === 'complete') {
        waitReady(30);
    } else {
        window.addEventListener('load', function() {
            setTimeout(function() { waitReady(30); }, 500);
        });
    }
});
`
}

func settingsJS() string {
	return `
(function() {
    function qbhInit() {
        if (!document.body) {
            document.addEventListener('DOMContentLoaded', qbhInit);
            return;
        }

        var style = document.createElement('style');
        style.textContent = ''
            + '#qbh-gear { position:fixed; bottom:8px; right:8px; z-index:99999; width:28px; height:28px; border-radius:50%; border:none; background:rgba(50,50,50,0.6); color:#aaa; font-size:16px; cursor:pointer; opacity:0.4; transition:opacity 0.2s; line-height:28px; text-align:center; padding:0; }'
            + '#qbh-gear:hover { opacity:1; color:#fff; }'
            + '#qbh-overlay { display:none; position:fixed; top:0; left:0; width:100%; height:100%; background:rgba(0,0,0,0.5); z-index:100000; justify-content:center; align-items:center; }'
            + '#qbh-modal { background:#2b2b2b; border-radius:8px; width:440px; max-width:90vw; box-shadow:0 8px 32px rgba(0,0,0,0.5); color:#e0e0e0; font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; font-size:14px; overflow:hidden; }'
            + '.qbh-header { display:flex; align-items:center; border-bottom:1px solid #444; }'
            + '.qbh-tab { flex:1; padding:12px; border:none; background:none; color:#888; cursor:pointer; font-size:14px; border-bottom:2px solid transparent; }'
            + '.qbh-tab.active { color:#4a9eff; border-bottom-color:#4a9eff; }'
            + '.qbh-tab:hover { color:#ccc; }'
            + '.qbh-close { width:40px; height:44px; border:none; background:none; color:#888; font-size:20px; cursor:pointer; }'
            + '.qbh-close:hover { color:#fff; }'
            + '.qbh-panel { padding:20px; display:none; }'
            + '.qbh-panel.active { display:block; }'
            + '.qbh-label { display:block; color:#999; font-size:11px; margin-bottom:6px; text-transform:uppercase; letter-spacing:0.5px; }'
            + '.qbh-input { width:100%; padding:8px 10px; background:#1e1e1e; border:1px solid #444; border-radius:4px; color:#e0e0e0; font-size:14px; box-sizing:border-box; outline:none; }'
            + '.qbh-input:focus { border-color:#4a9eff; }'
            + '.qbh-btn { padding:7px 14px; border:none; border-radius:4px; cursor:pointer; font-size:13px; }'
            + '.qbh-btn-p { background:#4a9eff; color:#fff; }'
            + '.qbh-btn-p:hover { background:#3a8eef; }'
            + '.qbh-btn-s { background:#444; color:#e0e0e0; }'
            + '.qbh-btn-s:hover { background:#555; }'
            + '.qbh-btn-d { background:#c44; color:#fff; }'
            + '.qbh-btn-d:hover { background:#b33; }'
            + '.qbh-section { margin-bottom:18px; }'
            + '.qbh-section:last-child { margin-bottom:0; }'
            + '.qbh-hr { border:none; border-top:1px solid #444; margin:18px 0; }'
            + '.qbh-row { display:flex; gap:8px; align-items:center; }'
            + '.qbh-row .qbh-input { flex:1; }'
            + '.qbh-msg { padding:8px 10px; border-radius:4px; margin-top:8px; font-size:13px; display:none; }'
            + '.qbh-about-row { margin-bottom:12px; }'
            + '.qbh-about-k { color:#888; font-size:12px; }'
            + '.qbh-about-v { color:#e0e0e0; font-size:15px; margin-top:2px; }';
        document.head.appendChild(style);

        var gear = document.createElement('button');
        gear.id = 'qbh-gear';
        gear.innerHTML = '⚙';
        gear.title = 'QBWebUIHelper Settings';
        document.body.appendChild(gear);

        var overlay = document.createElement('div');
        overlay.id = 'qbh-overlay';
        var modal = document.createElement('div');
        modal.id = 'qbh-modal';
        modal.innerHTML = '<div class="qbh-header">'
            + '<button class="qbh-tab active" data-tab="settings">Settings</button>'
            + '<button class="qbh-tab" data-tab="about">About</button>'
            + '<button class="qbh-close" id="qbh-x">×</button>'
            + '</div>'
            + '<div id="qbh-p-settings" class="qbh-panel active">'
            + '<div class="qbh-section">'
            + '<label class="qbh-label">WebUI URL</label>'
            + '<div class="qbh-row">'
            + '<input id="qbh-url" class="qbh-input" type="text" placeholder="http://...">'
            + '<button id="qbh-save-url" class="qbh-btn qbh-btn-p">Save</button>'
            + '</div>'
            + '<div id="qbh-url-msg" class="qbh-msg"></div>'
            + '</div>'
            + '<hr class="qbh-hr">'
            + '<div class="qbh-section">'
            + '<label class="qbh-label">File Associations</label>'
            + '<p id="qbh-reg-status" style="margin:4px 0 10px;font-size:13px;color:#999"></p>'
            + '<div class="qbh-row">'
            + '<button id="qbh-reg" class="qbh-btn qbh-btn-p">Register</button>'
            + '<button id="qbh-unreg" class="qbh-btn qbh-btn-d">Unregister</button>'
            + '<button id="qbh-defapps" class="qbh-btn qbh-btn-s">Default Apps</button>'
            + '</div>'
            + '<div id="qbh-reg-msg" class="qbh-msg"></div>'
            + '</div>'
            + '</div>'
            + '<div id="qbh-p-about" class="qbh-panel">'
            + '<div class="qbh-about-row"><div class="qbh-about-k">Application</div><div class="qbh-about-v">QBWebUIHelper</div></div>'
            + '<div class="qbh-about-row"><div class="qbh-about-k">Version</div><div class="qbh-about-v">1.0.0</div></div>'
            + '<div class="qbh-about-row"><div class="qbh-about-k">Author</div><div class="qbh-about-v">Kritblade</div></div>'
            + '<div class="qbh-about-row"><div class="qbh-about-k">License</div><div class="qbh-about-v">AGPL-3.0</div></div>'
            + '<hr class="qbh-hr">'
            + '<p style="color:#888;font-size:13px;line-height:1.5;margin:0">'
            + 'qBittorrent WebUI Desktop Wrapper.<br>Registers as default handler for .torrent files and magnet: links.</p>'
            + '</div>';
        overlay.appendChild(modal);
        document.body.appendChild(overlay);

        function show() {
            overlay.style.display = 'flex';
            __qbHelper_getURL().then(function(url) {
                document.getElementById('qbh-url').value = url;
            });
            refreshReg();
        }
        function hide() { overlay.style.display = 'none'; }
        function refreshReg() {
            __qbHelper_isRegistered().then(function(r) {
                var s = document.getElementById('qbh-reg-status');
                if (r) {
                    s.innerHTML = 'Status: <span style="color:#6c6">Registered</span>';
                } else {
                    s.innerHTML = 'Status: <span style="color:#c66">Not registered</span>';
                }
            });
        }
        function showMsg(id, text, ok) {
            var el = document.getElementById(id);
            el.textContent = text;
            el.style.display = 'block';
            el.style.background = ok ? '#1a3a1a' : '#1a2a3a';
            el.style.color = ok ? '#6c6' : '#6ac';
            el.style.border = '1px solid ' + (ok ? '#2a4a2a' : '#2a3a4a');
            setTimeout(function() { el.style.display = 'none'; }, 3000);
        }

        gear.onclick = show;
        overlay.onclick = function(e) { if (e.target === overlay) hide(); };
        document.getElementById('qbh-x').onclick = hide;
        document.addEventListener('keydown', function(e) {
            if (e.key === 'Escape' && overlay.style.display === 'flex') hide();
        });

        var tabs = modal.querySelectorAll('.qbh-tab');
        for (var i = 0; i < tabs.length; i++) {
            tabs[i].onclick = function() {
                for (var j = 0; j < tabs.length; j++) tabs[j].className = 'qbh-tab';
                this.className = 'qbh-tab active';
                var panels = modal.querySelectorAll('.qbh-panel');
                for (var j = 0; j < panels.length; j++) panels[j].className = 'qbh-panel';
                document.getElementById('qbh-p-' + this.getAttribute('data-tab')).className = 'qbh-panel active';
            };
        }

        document.getElementById('qbh-url').onkeydown = function(e) {
            if (e.key === 'Enter') document.getElementById('qbh-save-url').click();
        };

        document.getElementById('qbh-save-url').onclick = function() {
            var url = document.getElementById('qbh-url').value.trim();
            if (!url) return;
            __qbHelper_setURL(url).then(function() {
                showMsg('qbh-url-msg', 'Saved! Navigating...', true);
                setTimeout(function() { window.location.href = url; }, 500);
            });
        };

        document.getElementById('qbh-reg').onclick = function() {
            __qbHelper_register().then(function() {
                showMsg('qbh-reg-msg', 'Registered as .torrent and magnet: handler.', true);
                refreshReg();
            });
        };

        document.getElementById('qbh-unreg').onclick = function() {
            __qbHelper_unregister().then(function() {
                showMsg('qbh-reg-msg', 'File associations removed.', true);
                refreshReg();
            });
        };

        document.getElementById('qbh-defapps').onclick = function() {
            __qbHelper_openDefaultApps();
        };
    }
    qbhInit();
})();
`
}
