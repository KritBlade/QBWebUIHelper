//go:build !windows

package main

import (
	"fmt"
	"os"
	"os/exec"
	"runtime"
)

func allowForeground() {}

func bringToFront() {}

func registerAssociations() {
	fmt.Printf("File association registration is not yet supported on %s.\n", runtime.GOOS)
}

func unregisterAssociations() {
	fmt.Printf("Not supported on %s.\n", runtime.GOOS)
}

func isRegistered() bool { return false }

func openDefaultApps() {}

func showErrorDialog(msg string) {
	fmt.Fprintln(os.Stderr, msg)
	if runtime.GOOS == "darwin" {
		exec.Command("osascript", "-e",
			fmt.Sprintf(`display alert "QBWebUIHelper" message "%s" as critical`, msg)).Run()
	}
}
