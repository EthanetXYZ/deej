package deej

import (
	"os"

	"golang.org/x/sys/windows"
)

func redirectStderr(f *os.File) error {
	return windows.SetStdHandle(windows.STD_ERROR_HANDLE, windows.Handle(f.Fd()))
}
