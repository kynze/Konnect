"""Run a GUI program on its own, never-displayed Windows desktop (Windows only).

KiCad's IPC API needs the editor and KiCad has no headless mode, so the editor window is
created on a private desktop: nothing appears on the user's screen and nothing can take focus.
Named pipes are not bound to a desktop, so the editor's scratch API socket works as usual.
"""
import ctypes
from ctypes import wintypes
import subprocess

CREATE_UNICODE_ENVIRONMENT = 0x00000400
GENERIC_ALL = 0x10000000
WAIT_OBJECT_0 = 0


class STARTUPINFOW(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("lpReserved", wintypes.LPWSTR),
                ("lpDesktop", wintypes.LPWSTR), ("lpTitle", wintypes.LPWSTR),
                ("dwX", wintypes.DWORD), ("dwY", wintypes.DWORD),
                ("dwXSize", wintypes.DWORD), ("dwYSize", wintypes.DWORD),
                ("dwXCountChars", wintypes.DWORD), ("dwYCountChars", wintypes.DWORD),
                ("dwFillAttribute", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
                ("wShowWindow", wintypes.WORD), ("cbReserved2", wintypes.WORD),
                ("lpReserved2", ctypes.c_void_p), ("hStdInput", wintypes.HANDLE),
                ("hStdOutput", wintypes.HANDLE), ("hStdError", wintypes.HANDLE)]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [("hProcess", wintypes.HANDLE), ("hThread", wintypes.HANDLE),
                ("dwProcessId", wintypes.DWORD), ("dwThreadId", wintypes.DWORD)]


class HiddenProcess:
    def __init__(self, exe, args, cwd, env, desktop_name):
        k32 = self._k32 = ctypes.WinDLL("kernel32", use_last_error=True)
        u32 = self._u32 = ctypes.WinDLL("user32", use_last_error=True)
        k32.CreateProcessW.argtypes = [
            wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p, ctypes.c_void_p, wintypes.BOOL,
            wintypes.DWORD, ctypes.c_void_p, wintypes.LPCWSTR, ctypes.c_void_p, ctypes.c_void_p]
        k32.CloseHandle.argtypes = [wintypes.HANDLE]
        k32.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
        k32.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
        k32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        u32.CreateDesktopW.restype = wintypes.HANDLE
        u32.CreateDesktopW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.c_void_p,
                                       wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
        u32.CloseDesktop.argtypes = [wintypes.HANDLE]

        self.desktop = u32.CreateDesktopW(desktop_name, None, None, 0, GENERIC_ALL, None)
        assert self.desktop, f"could not create the hidden desktop: {ctypes.get_last_error()}"
        info = STARTUPINFOW()
        info.cb = ctypes.sizeof(info)
        info.lpDesktop = desktop_name
        proc = PROCESS_INFORMATION()
        block = "\x00".join(f"{k}={v}" for k, v in env.items()) + "\x00\x00"
        env_buffer = ctypes.create_unicode_buffer(block, len(block))
        cmdline = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(exe), *map(str, args)]))
        ok = k32.CreateProcessW(None, cmdline, None, None, False, CREATE_UNICODE_ENVIRONMENT,
                                env_buffer, str(cwd), ctypes.byref(info), ctypes.byref(proc))
        assert ok, f"CreateProcess failed: {ctypes.get_last_error()}"
        self._process = proc.hProcess
        k32.CloseHandle(proc.hThread)
        self.pid = proc.dwProcessId
        self.returncode = None

    def window_titles(self):
        """Titles of the top-level windows on the private desktop (for diagnosing a stuck editor)."""
        titles = []
        callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

        def collect(hwnd, _):
            length = self._u32.GetWindowTextLengthW(hwnd)
            buffer = ctypes.create_unicode_buffer(length + 1)
            self._u32.GetWindowTextW(hwnd, buffer, length + 1)
            if buffer.value:
                titles.append(buffer.value)
            return True

        self._u32.EnumDesktopWindows.argtypes = [wintypes.HANDLE, callback_type, wintypes.LPARAM]
        self._u32.EnumDesktopWindows(self.desktop, callback_type(collect), 0)
        return titles

    def poll(self):
        if self.returncode is None and \
                self._k32.WaitForSingleObject(self._process, 0) == WAIT_OBJECT_0:
            code = wintypes.DWORD()
            self._k32.GetExitCodeProcess(self._process, ctypes.byref(code))
            self.returncode = code.value
        return self.returncode

    def terminate(self):
        if self.poll() is None:
            self._k32.TerminateProcess(self._process, 1)

    def wait(self, timeout=15):
        self._k32.WaitForSingleObject(self._process, int(timeout * 1000))
        self.poll()
        self._k32.CloseHandle(self._process)
        self._u32.CloseDesktop(self.desktop)
