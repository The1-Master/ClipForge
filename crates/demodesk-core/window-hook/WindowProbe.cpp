#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <initializer_list>

static void resultLog(const char* message) {
    WCHAR path[32768];
    if (!GetEnvironmentVariableW(L"DEMODESK_WINDOW_HOOK_LOG", path, 32768)) return;
    HANDLE file = CreateFileW(path, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (file == INVALID_HANDLE_VALUE) return;
    DWORD written;
    WriteFile(file, message, (DWORD)lstrlenA(message), &written, NULL);
    WriteFile(file, "\n", 1, &written, NULL);
    CloseHandle(file);
}
static int failure(const char* message) { resultLog(message); fprintf(stderr, "%s\n", message); return 1; }
static bool emulateSdlClamp = false;
// Reproduce SDL's own non-resizable handler: it bypasses DefWindowProc and
// clamps maximum bounds to the initial size unless the owned subclass fixes it.
static LRESULT CALLBACK SdlWindowProc(HWND window, UINT message, WPARAM w, LPARAM l) {
    if (emulateSdlClamp && message == WM_GETMINMAXINFO && l) {
        MINMAXINFO* limits = reinterpret_cast<MINMAXINFO*>(l);
        limits->ptMaxSize = {640, 480};
        limits->ptMaxTrackSize = {640, 480};
        return 0;
    }
    if (message == WM_APP + 71) return 71;
    return DefWindowProcW(window, message, w, l);
}
int wmain(int argc, wchar_t** argv) {
    if (argc != 2) return failure("expected hook DLL");
    emulateSdlClamp = GetEnvironmentVariableW(L"CLIPFORGE_RENDER_WIDTH", NULL, 0) > 0;
    HMODULE hook = lstrcmpW(argv[1], L"--injected") == 0 ? GetModuleHandleW(L"demodesk-window-hook.dll") : LoadLibraryW(argv[1]);
    if (!hook) return failure("LoadLibrary hook failed");
    POINT cursorBefore, cursorAfter;
    if (!GetCursorPos(&cursorBefore)) return failure("GetCursorPos failed");
    if (!SetCursorPos(cursorBefore.x + 2, cursorBefore.y + 2) || !GetCursorPos(&cursorAfter)) return failure("cursor request failed");
    if (cursorBefore.x != cursorAfter.x || cursorBefore.y != cursorAfter.y) {
        SetCursorPos(cursorBefore.x, cursorBefore.y);
        return failure("hidden process moved the cursor");
    }
    RECT clipBefore, clipAfter;
    if (!GetClipCursor(&clipBefore)) return failure("GetClipCursor failed");
    RECT requestedClip = { cursorBefore.x, cursorBefore.y, cursorBefore.x + 10, cursorBefore.y + 10 };
    if (!ClipCursor(&requestedClip) || !GetClipCursor(&clipAfter)) return failure("clip request failed");
    if (!EqualRect(&clipBefore, &clipAfter)) {
        ClipCursor(&clipBefore);
        return failure("hidden process changed cursor clipping");
    }
    if (!ClipCursor(NULL) || !GetClipCursor(&clipAfter) || !EqualRect(&clipBefore, &clipAfter))
        return failure("hidden process released cursor clipping");
    // CS2 starts threads with small stacks; DLL_THREAD_ATTACH must fit them too.
    HANDLE smallThread = CreateThread(NULL, 65536, [](LPVOID) -> DWORD { return 0; }, NULL, STACK_SIZE_PARAM_IS_A_RESERVATION, NULL);
    if (!smallThread || WaitForSingleObject(smallThread, 5000) != WAIT_OBJECT_0) return failure("small-stack thread initialization failed");
    CloseHandle(smallThread);
    WNDCLASSW cls = {};
    cls.hInstance = GetModuleHandleW(NULL);
    cls.lpfnWndProc = SdlWindowProc;
    cls.lpszClassName = L"SDL_app";
    if (!RegisterClassW(&cls)) return failure("RegisterClass failed");
    HWND window = CreateWindowExW(WS_EX_TOOLWINDOW, cls.lpszClassName, L"DemoDesk hook test",
        WS_OVERLAPPEDWINDOW | WS_VISIBLE, -32000, -32000, 640, 480,
        NULL, NULL, cls.hInstance, NULL);
    if (!window || IsWindowVisible(window)) return failure("visible creation was not blocked");
    if (SendMessageW(window, WM_APP + 71, 0, 0) != 71) return failure("SDL handler return value changed");
    if (!(GetWindowLongPtrW(window, GWL_EXSTYLE) & WS_EX_NOACTIVATE)) return failure("missing no-activate style");
    const HWND foregroundBefore = GetForegroundWindow();
    const HWND focusBefore = GetFocus();
    const HWND activeBefore = GetActiveWindow();
    SetForegroundWindow(window);
    SetFocus(window);
    SetActiveWindow(window);
    HWND child = CreateWindowExW(0, L"STATIC", L"Child", WS_CHILD, 0, 0, 10, 10,
        window, NULL, cls.hInstance, NULL);
    if (!child) return failure("child creation failed");
    SetFocus(child);
    if (GetForegroundWindow() != foregroundBefore || GetFocus() != focusBefore || GetActiveWindow() != activeBefore)
        return failure("hidden window changed focus or activation");
    DestroyWindow(child);
    for (int mode : { SW_SHOW, SW_RESTORE, SW_SHOWDEFAULT, SW_SHOWMAXIMIZED }) {
        if (ShowWindow(window, mode) || IsWindowVisible(window)) return failure("ShowWindow failed");
    }
    if (!ShowWindowAsync(window, SW_SHOW) || IsWindowVisible(window)) return failure("ShowWindowAsync failed");
    if (!SetWindowPos(window, NULL, -32000, -32000, 800, 600, SWP_SHOWWINDOW | SWP_NOZORDER))
        return failure("SetWindowPos failed");
    RECT rect;
    WCHAR requestedSize[16];
    const DWORD requestedWidthLength = GetEnvironmentVariableW(L"CLIPFORGE_RENDER_WIDTH", requestedSize, 16);
    const int requestedWidth = requestedWidthLength > 0 && requestedWidthLength < 16 ? _wtoi(requestedSize) : 0;
    const DWORD requestedHeightLength = GetEnvironmentVariableW(L"CLIPFORGE_RENDER_HEIGHT", requestedSize, 16);
    const int requestedHeight = requestedHeightLength > 0 && requestedHeightLength < 16 ? _wtoi(requestedSize) : 0;
    if (requestedWidth >= 640 && requestedWidth <= 7680 && requestedHeight >= 360 && requestedHeight <= 4320) {
        if (IsWindowVisible(window) || !GetClientRect(window, &rect) ||
            rect.right - rect.left != requestedWidth || rect.bottom - rect.top != requestedHeight) {
            char detail[128];
            sprintf_s(detail, "requested %dx%d client, actual %ldx%ld", requestedWidth, requestedHeight,
                rect.right - rect.left, rect.bottom - rect.top);
            resultLog(detail);
            return failure("requested capture client size was not preserved");
        }
        resultLog("requested capture client size preserved");
        MINMAXINFO limits = {};
        SendMessageW(window, WM_GETMINMAXINFO, 0, reinterpret_cast<LPARAM>(&limits));
        if (limits.ptMaxTrackSize.x < requestedWidth || limits.ptMaxTrackSize.y < requestedHeight
            || limits.ptMaxSize.x < requestedWidth || limits.ptMaxSize.y < requestedHeight)
            return failure("custom SDL handler still clamps tracking bounds");
        resultLog("custom SDL tracking handler preserved capture bounds");
    } else if (IsWindowVisible(window) || !GetWindowRect(window, &rect) ||
        rect.right - rect.left != 800 || rect.bottom - rect.top != 600) {
        return failure("hidden resize was not preserved");
    }
    HWND ansi = CreateWindowExA(WS_EX_TOOLWINDOW, "SDL_app", "ANSI test", WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        -32000, -32000, 320, 240, NULL, NULL, cls.hInstance, NULL);
    if (!ansi || IsWindowVisible(ansi)) return failure("ANSI create failed");
    HWND other = CreateWindowExW(WS_EX_TOOLWINDOW, L"STATIC", L"Other",
        WS_OVERLAPPEDWINDOW | WS_VISIBLE, -32000, -32000, 320, 240,
        NULL, NULL, cls.hInstance, NULL);
    if (!other || !IsWindowVisible(other)) return failure("unrelated class changed");
    DestroyWindow(other);
    DestroyWindow(ansi);
    if (!FreeLibrary(hook)) return failure("unload failed");
    if (reinterpret_cast<WNDPROC>(GetWindowLongPtrW(window, GWLP_WNDPROC)) != SdlWindowProc
        || SendMessageW(window, WM_APP + 71, 0, 0) != 71)
        return failure("original SDL handler was not restored on unload");
    ShowWindow(window, SW_SHOW);
    if (!IsWindowVisible(window)) return failure("hooks were not detached");
    DestroyWindow(window);
    resultLog("probe passed");
    puts("Win32 create/show/async/position/resize, non-target and unload checks passed");
    return 0;
}
