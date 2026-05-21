//go:build windows

package main

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
	"syscall"
	"unsafe"
)

var (
	user32            = syscall.NewLazyDLL("user32.dll")
	shcore            = syscall.NewLazyDLL("shcore.dll")
	procShowWindow    = user32.NewProc("ShowWindow")
	procSetForeground = user32.NewProc("SetForegroundWindow")
)

func init() {
	// PROCESS_PER_MONITOR_DPI_AWARE = 2 — makes text render sharp on high-DPI displays
	proc := shcore.NewProc("SetProcessDpiAwareness")
	if proc.Find() == nil {
		proc.Call(2)
	}
}

func allowForeground() {
	proc := user32.NewProc("AllowSetForegroundWindow")
	proc.Call(uintptr(0xFFFFFFFF)) // ASFW_ANY — let any process take foreground
}

func bringToFront() {
	if wv == nil {
		return
	}
	hwnd := uintptr(wv.Window())
	if hwnd == 0 {
		return
	}
	procShowWindow.Call(hwnd, 9) // SW_RESTORE
	procSetForeground.Call(hwnd)
}

func registerAssociations() {
	exe, err := os.Executable()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		return
	}

	cmd := fmt.Sprintf(`"%s" "%%1"`, exe)
	icon := fmt.Sprintf(`"%s",0`, exe)

	reg := func(args ...string) {
		if err := exec.Command("reg", args...).Run(); err != nil {
			fmt.Printf("  Warning: %v\n", err)
		}
	}

	fmt.Println("Registering file associations...")

	reg("add", `HKCU\Software\Classes\QBWebUIHelper.Torrent`, "/ve", "/d", "Torrent File", "/f")
	reg("add", `HKCU\Software\Classes\QBWebUIHelper.Torrent\DefaultIcon`, "/ve", "/d", icon, "/f")
	reg("add", `HKCU\Software\Classes\QBWebUIHelper.Torrent\shell\open\command`, "/ve", "/d", cmd, "/f")

	reg("add", `HKCU\Software\Classes\QBWebUIHelper.Magnet`, "/ve", "/d", "Magnet Link", "/f")
	reg("add", `HKCU\Software\Classes\QBWebUIHelper.Magnet`, "/v", "URL Protocol", "/d", "", "/f")
	reg("add", `HKCU\Software\Classes\QBWebUIHelper.Magnet\DefaultIcon`, "/ve", "/d", icon, "/f")
	reg("add", `HKCU\Software\Classes\QBWebUIHelper.Magnet\shell\open\command`, "/ve", "/d", cmd, "/f")

	reg("add", `HKCU\Software\Classes\.torrent`, "/ve", "/d", "QBWebUIHelper.Torrent", "/f")

	reg("add", `HKCU\Software\Classes\magnet`, "/ve", "/d", "URL:Magnet Protocol", "/f")
	reg("add", `HKCU\Software\Classes\magnet`, "/v", "URL Protocol", "/d", "", "/f")
	reg("add", `HKCU\Software\Classes\magnet\DefaultIcon`, "/ve", "/d", icon, "/f")
	reg("add", `HKCU\Software\Classes\magnet\shell\open\command`, "/ve", "/d", cmd, "/f")

	reg("add", `HKCU\Software\QBWebUIHelper\Capabilities`, "/v", "ApplicationName", "/d", "QBWebUIHelper", "/f")
	reg("add", `HKCU\Software\QBWebUIHelper\Capabilities`, "/v", "ApplicationDescription", "/d", "qBittorrent WebUI Desktop Wrapper", "/f")
	reg("add", `HKCU\Software\QBWebUIHelper\Capabilities\FileAssociations`, "/v", ".torrent", "/d", "QBWebUIHelper.Torrent", "/f")
	reg("add", `HKCU\Software\QBWebUIHelper\Capabilities\URLAssociations`, "/v", "magnet", "/d", "QBWebUIHelper.Magnet", "/f")

	reg("add", `HKCU\Software\RegisteredApplications`, "/v", "QBWebUIHelper", "/d", `Software\QBWebUIHelper\Capabilities`, "/f")

	notifyAssocChanged()

	fmt.Println("Done! Registered:")
	fmt.Println("  .torrent files -> QBWebUIHelper")
	fmt.Println("  magnet: links  -> QBWebUIHelper")
	fmt.Println()
	fmt.Println("Open Settings > Apps > Default apps to set QBWebUIHelper as default.")
}

func unregisterAssociations() {
	for _, key := range []string{
		`HKCU\Software\Classes\.torrent`,
		`HKCU\Software\Classes\QBWebUIHelper`,
		`HKCU\Software\Classes\QBWebUIHelper.Torrent`,
		`HKCU\Software\Classes\QBWebUIHelper.Magnet`,
		`HKCU\Software\Classes\magnet`,
		`HKCU\Software\QBWebUIHelper`,
	} {
		exec.Command("reg", "delete", key, "/f").Run()
	}
	exec.Command("reg", "delete", `HKCU\Software\RegisteredApplications`, "/v", "QBWebUIHelper", "/f").Run()
	notifyAssocChanged()
	fmt.Println("File associations removed.")
}

func isRegistered() bool {
	return exec.Command("reg", "query", `HKCU\Software\RegisteredApplications`, "/v", "QBWebUIHelper").Run() == nil
}

func openDefaultApps() {
	exec.Command("cmd", "/c", "start", "ms-settings:defaultapps").Run()
}

func notifyAssocChanged() {
	shell32 := syscall.NewLazyDLL("shell32.dll")
	proc := shell32.NewProc("SHChangeNotify")
	proc.Call(0x08000000, 0, 0, 0)
}

func showErrorDialog(msg string) {
	escaped := strings.ReplaceAll(msg, "'", "''")
	escaped = strings.ReplaceAll(escaped, "\n", "`n")
	script := fmt.Sprintf(
		`Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('%s','QBWebUIHelper','OK','Error')`,
		escaped)
	exec.Command("powershell", "-NoProfile", "-Command", script).Run()
}

// ensure unsafe is used (for wv.Window() -> uintptr conversion)
var _ unsafe.Pointer
