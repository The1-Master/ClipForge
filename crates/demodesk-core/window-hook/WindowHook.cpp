#include <windows.h>
#include <stdlib.h>
#include <stdio.h>
#include "detours.h"

// Window hooks target SDL; cursor hooks affect only this hidden game process.
static HANDLE logFile = INVALID_HANDLE_VALUE;
static volatile LONG recorded[10] = {};
static volatile LONG observedClasses = 0;
// Only this owned recording process inherits these values. No desktop/display mode change.
static int captureWidth = 0;
static int captureHeight = 0;
static SRWLOCK windowLock = SRWLOCK_INIT;
struct OwnedWindowProc { HWND window; WNDPROC original; bool unicode; };
static OwnedWindowProc ownedWindows[32] = {};
static volatile LONG reportedClientSize = 0;
static void CaptureOuterSize(DWORD style, DWORD ex, int& w, int& h) {
    if (captureWidth <= 0 || captureHeight <= 0) return;
    RECT rectangle{0, 0, captureWidth, captureHeight};
    if (AdjustWindowRectEx(&rectangle, style, FALSE, ex)) {
        w = rectangle.right - rectangle.left;
        h = rectangle.bottom - rectangle.top;
    }
}
static decltype(&CreateWindowExW) realCreateW = CreateWindowExW;
static decltype(&CreateWindowExA) realCreateA = CreateWindowExA;
static decltype(&ShowWindow) realShow = ShowWindow;
static decltype(&ShowWindowAsync) realShowAsync = ShowWindowAsync;
static decltype(&SetWindowPos) realSetPos = SetWindowPos;
static decltype(&SetCursorPos) realSetCursor = SetCursorPos;
static decltype(&ClipCursor) realClipCursor = ClipCursor;
static decltype(&SetForegroundWindow) realForeground = SetForegroundWindow;
static decltype(&SetFocus) realFocus = SetFocus;
static decltype(&SetActiveWindow) realActive = SetActiveWindow;
static decltype(&DefWindowProcW) realDefW = DefWindowProcW;
static decltype(&DefWindowProcA) realDefA = DefWindowProcA;

static void Log(const char* text) {
    if (logFile == INVALID_HANDLE_VALUE) return;
    DWORD written;
    WriteFile(logFile, text, (DWORD)lstrlenA(text), &written, NULL);
}
static void Once(int slot, const char* text) {
    if (InterlockedCompareExchange(&recorded[slot], 1, 0) == 0) Log(text);
}
// Record a bounded class sample so an unexpected Valve class needs no extra diagnostic run.
static void ObserveClass(LPCWSTR name, DWORD style) {
    if ((style & WS_CHILD) || InterlockedIncrement(&observedClasses) > 12) return;
    char text[256];
    if (IS_INTRESOURCE(name)) { Log("create class=<atom>\n"); return; }
    if (WideCharToMultiByte(CP_UTF8, 0, name, -1, text, 256, NULL, NULL)) {
        Log("create class="); Log(text); Log("\n");
    }
}
static bool IsClassW(LPCWSTR name) {
    return !IS_INTRESOURCE(name) && lstrcmpiW(name, L"SDL_app") == 0;
}
static bool IsClassA(LPCSTR name) {
    return !IS_INTRESOURCE(name) && lstrcmpiA(name, "SDL_app") == 0;
}
static bool IsGameWindow(HWND window) {
    DWORD pid = 0;
    WCHAR name[128];
    return GetWindowThreadProcessId(window, &pid) && pid == GetCurrentProcessId()
        && !(GetWindowLongPtrW(window, GWL_STYLE) & WS_CHILD)
        && GetClassNameW(window, name, 128) && IsClassW(name);
}
// The default Win32 maximum tracking size otherwise clamps a window to this 1080p desktop.
// Extend that limit only for SDL windows in this owned process; never change a monitor mode.
static void CaptureTrackingSize(HWND window, UINT message, LPARAM parameter) {
    if (message != WM_GETMINMAXINFO || !parameter || !IsGameWindow(window) || captureWidth <= 0) return;
    int width = captureWidth, height = captureHeight;
    CaptureOuterSize((DWORD)GetWindowLongPtrW(window, GWL_STYLE),
        (DWORD)GetWindowLongPtrW(window, GWL_EXSTYLE), width, height);
    MINMAXINFO* limits = reinterpret_cast<MINMAXINFO*>(parameter);
    if (limits->ptMaxTrackSize.x < width) limits->ptMaxTrackSize.x = width;
    if (limits->ptMaxTrackSize.y < height) limits->ptMaxTrackSize.y = height;
    if (limits->ptMaxSize.x < width) limits->ptMaxSize.x = width;
    if (limits->ptMaxSize.y < height) limits->ptMaxSize.y = height;
}
// SDL handles WM_GETMINMAXINFO itself and can return without DefWindowProc.
// Apply the owned limit after its handler too, preserving every other message.
static LRESULT CALLBACK CaptureWindowProc(HWND window, UINT message, WPARAM w, LPARAM l) {
    OwnedWindowProc entry = {};
    AcquireSRWLockShared(&windowLock);
    for (const auto& candidate : ownedWindows) {
        if (candidate.window == window) { entry = candidate; break; }
    }
    ReleaseSRWLockShared(&windowLock);
    LRESULT result = entry.original
        ? (entry.unicode ? CallWindowProcW(entry.original, window, message, w, l)
                         : CallWindowProcA(entry.original, window, message, w, l))
        : realDefW(window, message, w, l);
    CaptureTrackingSize(window, message, l);
    if (message == WM_NCDESTROY) {
        AcquireSRWLockExclusive(&windowLock);
        for (auto& candidate : ownedWindows) {
            if (candidate.window == window) candidate = {};
        }
        ReleaseSRWLockExclusive(&windowLock);
    }
    return result;
}
static bool InstallWindowProc(HWND window) {
    if (!IsGameWindow(window) || captureWidth <= 0) return false;
    const WNDPROC previous = reinterpret_cast<WNDPROC>(GetWindowLongPtrW(window, GWLP_WNDPROC));
    if (!previous || previous == CaptureWindowProc) return previous != NULL;
    int slot = -1;
    AcquireSRWLockExclusive(&windowLock);
    for (int i = 0; i < 32; ++i) {
        if (!ownedWindows[i].window) {
            slot = i;
            ownedWindows[i] = {window, previous, IsWindowUnicode(window) != FALSE};
            break;
        }
    }
    ReleaseSRWLockExclusive(&windowLock);
    if (slot < 0) { Log("owned SDL window registry full\n"); return false; }
    SetLastError(0);
    const LONG_PTR replaced = SetWindowLongPtrW(window, GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(CaptureWindowProc));
    if (!replaced && GetLastError()) {
        AcquireSRWLockExclusive(&windowLock);
        if (ownedWindows[slot].window == window) ownedWindows[slot] = {};
        ReleaseSRWLockExclusive(&windowLock);
        Log("owned SDL window subclass failed\n");
        return false;
    }
    // SetWindowLongPtr returns the actual previous procedure. A subclass that
    // raced the initial read must remain in the forwarding/restoration chain.
    if (replaced) {
        AcquireSRWLockExclusive(&windowLock);
        if (ownedWindows[slot].window == window)
            ownedWindows[slot].original = reinterpret_cast<WNDPROC>(replaced);
        ReleaseSRWLockExclusive(&windowLock);
    }
    Log("owned SDL window subclass installed\n");
    return true;
}
static void RestoreWindowProcs() {
    // Explicit DLL unload in the synthetic probe; game process exit needs no UI work.
    OwnedWindowProc snapshot[32] = {};
    AcquireSRWLockShared(&windowLock);
    for (int i = 0; i < 32; ++i) snapshot[i] = ownedWindows[i];
    ReleaseSRWLockShared(&windowLock);
    for (const auto& candidate : snapshot) {
        if (candidate.window && IsGameWindow(candidate.window)
            && reinterpret_cast<WNDPROC>(GetWindowLongPtrW(candidate.window, GWLP_WNDPROC)) == CaptureWindowProc) {
            SetWindowLongPtrW(candidate.window, GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(candidate.original));
        }
    }
    AcquireSRWLockExclusive(&windowLock);
    for (auto& candidate : ownedWindows) candidate = {};
    ReleaseSRWLockExclusive(&windowLock);
}
static void ObserveClient(HWND window) {
    if (!IsGameWindow(window) || InterlockedIncrement(&reportedClientSize) > 8) return;
    RECT rectangle = {};
    if (GetClientRect(window, &rectangle)) {
        char text[128];
        sprintf_s(text, "owned SDL client=%ldx%ld requested=%dx%d\n",
            rectangle.right - rectangle.left, rectangle.bottom - rectangle.top, captureWidth, captureHeight);
        Log(text);
    }
}
static LRESULT WINAPI CaptureDefW(HWND window, UINT message, WPARAM w, LPARAM l) {
    LRESULT result = realDefW(window, message, w, l);
    CaptureTrackingSize(window, message, l);
    return result;
}
static LRESULT WINAPI CaptureDefA(HWND window, UINT message, WPARAM w, LPARAM l) {
    LRESULT result = realDefA(window, message, w, l);
    CaptureTrackingSize(window, message, l);
    return result;
}
static HWND WINAPI HiddenCreateW(DWORD ex, LPCWSTR cls, LPCWSTR title, DWORD style,
    int x, int y, int w, int h, HWND parent, HMENU menu, HINSTANCE instance, LPVOID param) {
    ObserveClass(cls, style);
    if (!(style & WS_CHILD) && IsClassW(cls)) {
        CaptureOuterSize(style, ex, w, h);
        style &= ~WS_VISIBLE;
        ex |= WS_EX_NOACTIVATE;
        Once(0, "intercepted CreateWindowExW SDL_app\n");
    }
    HWND window = realCreateW(ex, cls, title, style, x, y, w, h, parent, menu, instance, param);
    if (window && InstallWindowProc(window)) {
        // Creation may already have been clamped before the subclass existed.
        realSetPos(window, NULL, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_HIDEWINDOW | SWP_NOACTIVATE);
    }
    ObserveClient(window);
    return window;
}
static HWND WINAPI HiddenCreateA(DWORD ex, LPCSTR cls, LPCSTR title, DWORD style,
    int x, int y, int w, int h, HWND parent, HMENU menu, HINSTANCE instance, LPVOID param) {
    if (!(style & WS_CHILD) && IsClassA(cls)) {
        CaptureOuterSize(style, ex, w, h);
        style &= ~WS_VISIBLE;
        ex |= WS_EX_NOACTIVATE;
        Once(1, "intercepted CreateWindowExA SDL_app\n");
    }
    HWND window = realCreateA(ex, cls, title, style, x, y, w, h, parent, menu, instance, param);
    if (window && InstallWindowProc(window)) {
        realSetPos(window, NULL, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_HIDEWINDOW | SWP_NOACTIVATE);
    }
    ObserveClient(window);
    return window;
}
static void EnsureHidden(HWND window) {
    if (IsWindowVisible(window)) {
        realSetPos(window, NULL, 0, 0, 0, 0,
            SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}
static BOOL WINAPI HiddenShow(HWND window, int command) {
    if (!IsGameWindow(window)) return realShow(window, command);
    Once(2, "intercepted ShowWindow SDL_app\n");
    const BOOL wasVisible = IsWindowVisible(window);
    // Do not call ShowWindow: STARTUPINFO can override even an initial SW_HIDE.
    EnsureHidden(window);
    return wasVisible;
}
static BOOL WINAPI HiddenShowAsync(HWND window, int command) {
    if (!IsGameWindow(window)) return realShowAsync(window, command);
    Once(3, "intercepted ShowWindowAsync SDL_app\n");
    EnsureHidden(window);
    return TRUE;
}
static BOOL WINAPI HiddenSetPos(HWND window, HWND after, int x, int y, int w, int h, UINT flags) {
    if (IsGameWindow(window)) {
        if (!(flags & SWP_NOSIZE)) {
            CaptureOuterSize((DWORD)GetWindowLongPtrW(window, GWL_STYLE),
                (DWORD)GetWindowLongPtrW(window, GWL_EXSTYLE), w, h);
        }
        Once(4, "intercepted SetWindowPos SDL_app\n");
        flags = (flags & ~SWP_SHOWWINDOW) | SWP_HIDEWINDOW | SWP_NOACTIVATE;
    }
    BOOL result = realSetPos(window, after, x, y, w, h, flags);
    ObserveClient(window);
    return result;
}

// Hidden playback must not warp the desktop pointer or change another app's clip.
static BOOL WINAPI HiddenSetCursor(int, int) {
    Once(5, "intercepted SetCursorPos\n");
    return TRUE;
}
static BOOL WINAPI HiddenClipCursor(const RECT*) {
    Once(6, "intercepted ClipCursor\n");
    return TRUE;
}

// Focus can activate even an invisible window. Keep the existing owner untouched.
static bool IsGameFocusTarget(HWND window) {
    return window && IsGameWindow(GetAncestor(window, GA_ROOT));
}
static BOOL WINAPI HiddenForeground(HWND window) {
    if (!IsGameFocusTarget(window)) return realForeground(window);
    Once(7, "intercepted SetForegroundWindow SDL_app\n");
    return FALSE;
}
static HWND WINAPI HiddenFocus(HWND window) {
    if (!IsGameFocusTarget(window)) return realFocus(window);
    Once(8, "intercepted SetFocus SDL_app\n");
    return GetFocus();
}
static HWND WINAPI HiddenActive(HWND window) {
    if (!IsGameFocusTarget(window)) return realActive(window);
    Once(9, "intercepted SetActiveWindow SDL_app\n");
    return GetActiveWindow();
}

static LONG Hooks(bool attach) {
    LONG error = DetourTransactionBegin();
    if (error != NO_ERROR) return error;
    error = DetourUpdateThread(GetCurrentThread());
#define CHANGE(real, replacement) if (error == NO_ERROR) error = attach ? \
    DetourAttach(&(PVOID&)real, replacement) : DetourDetach(&(PVOID&)real, replacement)
    CHANGE(realCreateW, HiddenCreateW);
    CHANGE(realCreateA, HiddenCreateA);
    CHANGE(realShow, HiddenShow);
    CHANGE(realShowAsync, HiddenShowAsync);
    CHANGE(realSetPos, HiddenSetPos);
    CHANGE(realSetCursor, HiddenSetCursor);
    CHANGE(realClipCursor, HiddenClipCursor);
    CHANGE(realForeground, HiddenForeground);
    CHANGE(realFocus, HiddenFocus);
    CHANGE(realActive, HiddenActive);
    CHANGE(realDefW, CaptureDefW);
    CHANGE(realDefA, CaptureDefA);
#undef CHANGE
    if (error != NO_ERROR) { DetourTransactionAbort(); return error; }
    return DetourTransactionCommit();
}

BOOL WINAPI DllMain(HINSTANCE, DWORD reason, LPVOID reserved) {
    if (reason == DLL_PROCESS_ATTACH) {
        WCHAR requested[16];
        DWORD count = GetEnvironmentVariableW(L"CLIPFORGE_RENDER_WIDTH", requested, 16);
        if (count > 0 && count < 16) captureWidth = _wtoi(requested);
        count = GetEnvironmentVariableW(L"CLIPFORGE_RENDER_HEIGHT", requested, 16);
        if (count > 0 && count < 16) captureHeight = _wtoi(requested);
        if (captureWidth < 640 || captureWidth > 7680 || captureHeight < 360 || captureHeight > 4320) {
            captureWidth = captureHeight = 0;
        }
        // DllMain also runs on small-stack game threads; keep this buffer off their stacks.
        static WCHAR path[32768];
        DWORD length = GetEnvironmentVariableW(L"DEMODESK_WINDOW_HOOK_LOG", path, 32768);
        if (!length || length >= 32768) return FALSE;
        logFile = CreateFileW(path, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
            NULL, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
        if (logFile == INVALID_HANDLE_VALUE) return FALSE;
        // HLAE loads this DLL while the game's main thread is suspended.
        // No worker, LoadLibrary, or waiting under the loader lock.
        if (Hooks(true) != NO_ERROR) {
            Log("installation failed\n");
            CloseHandle(logFile);
            logFile = INVALID_HANDLE_VALUE;
            return FALSE;
        }
        Log("installed\n");
    } else if (reason == DLL_PROCESS_DETACH) {
        if (!reserved) { RestoreWindowProcs(); Hooks(false); }
        if (logFile != INVALID_HANDLE_VALUE) CloseHandle(logFile);
    }
    return TRUE;
}
