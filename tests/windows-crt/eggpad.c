#include <windows.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

#define EGGPAD_CAPACITY 4096
#define EGGPAD_LINE 256
#define EGGPAD_MARGIN 8
#define EGGPAD_STATUS 64
#define EGGPAD_CLIENT_WIDTH 400
#define EGGPAD_CLIENT_HEIGHT 300
#define EGGPAD_BORDERS 2
#define EGGPAD_TITLE_BAR 28
#define EGGPAD_CONTROL_S 0x13

static const wchar_t eggpad_class[] = L"EggPad";
static const wchar_t eggpad_default_path[] = L"C:\\tmp\\eggpad.txt";

static wchar_t path[MAX_PATH];
static char *text;
static size_t length;
static int typed;
static wchar_t status[EGGPAD_STATUS];

static void say_exit(void)
{
    printf("EGGPAD exit\n");
}

static void open_file(void)
{
    FILE *file = _wfopen(path, L"r");
    if (file == NULL) {
        printf("EGGPAD open failed %d\n", errno);
        length = 0;
        swprintf(status, EGGPAD_STATUS, L"New file");
        return;
    }
    length = fread(text, 1, EGGPAD_CAPACITY - 1, file);
    fclose(file);
    printf("EGGPAD opened %zu bytes\n", length);
    swprintf(status, EGGPAD_STATUS, L"%zu bytes", length);
}

static void save_file(void)
{
    FILE *file = _wfopen(path, L"w");
    long size;
    if (file == NULL) {
        printf("EGGPAD save failed %d\n", errno);
        swprintf(status, EGGPAD_STATUS, L"Not saved");
        return;
    }
    if (fwrite(text, 1, length, file) != length) {
        printf("EGGPAD write failed %d\n", errno);
    }
    size = ftell(file);
    if (fclose(file) != 0) {
        printf("EGGPAD close failed %d\n", errno);
    }
    printf("EGGPAD saved %ld bytes\n", size);
    swprintf(status, EGGPAD_STATUS, L"Saved %ld bytes", size);
}

static void append(char character)
{
    if (length < EGGPAD_CAPACITY - 1) {
        text[length] = character;
        length++;
        typed++;
    }
}

static void type_character(HWND window, wchar_t character)
{
    if (character == EGGPAD_CONTROL_S) {
        save_file();
    } else if (character == L'\b') {
        if (length > 0) {
            length--;
            typed++;
        }
    } else if (character == L'\r') {
        append('\n');
    } else if (character >= L' ' && character < 0x7f) {
        append((char)character);
    } else {
        return;
    }
    InvalidateRect(window, NULL, TRUE);
}

static void press_key(HWND window, WPARAM key)
{
    if (key == VK_F5) {
        open_file();
        InvalidateRect(window, NULL, TRUE);
    }
}

static void paint(HWND window)
{
    PAINTSTRUCT painting;
    RECT client;
    SIZE cell;
    wchar_t line[EGGPAD_LINE];
    int count = 0;
    int y = EGGPAD_MARGIN;
    size_t at;
    HDC dc = BeginPaint(window, &painting);
    GetClientRect(window, &client);
    GetTextExtentPoint32W(dc, L"M", 1, &cell);
    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, RGB(0, 0, 0));
    for (at = 0; at <= length; at++) {
        if (at == length || text[at] == '\n') {
            TextOutW(dc, EGGPAD_MARGIN, y, line, count);
            y += cell.cy;
            count = 0;
        } else if (count < EGGPAD_LINE) {
            line[count] = (wchar_t)(unsigned char)text[at];
            count++;
        }
    }
    TextOutW(dc, EGGPAD_MARGIN, client.bottom - cell.cy - EGGPAD_MARGIN, status,
             (int)wcslen(status));
    EndPaint(window, &painting);
}

static LRESULT CALLBACK window_procedure(HWND window, UINT message, WPARAM wparam,
                                         LPARAM lparam)
{
    switch (message) {
    case WM_CHAR:
        type_character(window, (wchar_t)wparam);
        return 0;
    case WM_KEYDOWN:
        press_key(window, wparam);
        return 0;
    case WM_PAINT:
        paint(window);
        return 0;
    case WM_DESTROY:
        printf("EGGPAD closed typed %d\n", typed);
        PostQuitMessage(0);
        return 0;
    default:
        return DefWindowProcW(window, message, wparam, lparam);
    }
}

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE previous, PWSTR command_line, int show)
{
    WNDCLASSEXW window_class;
    HWND window;
    MSG message;
    (void)previous;
    if (atexit(say_exit) != 0) {
        printf("EGGPAD atexit failed\n");
    }
    printf("EGGPAD arguments %d %ls\n", __argc, command_line);
    if (wcscpy_s(path, MAX_PATH, command_line[0] != L'\0' ? command_line : eggpad_default_path)
        != 0) {
        printf("EGGPAD path too long\n");
        return 1;
    }
    text = (char *)calloc(EGGPAD_CAPACITY, 1);
    if (text == NULL) {
        printf("EGGPAD out of memory\n");
        return 1;
    }
    open_file();
    memset(&window_class, 0, sizeof window_class);
    window_class.cbSize = sizeof window_class;
    window_class.lpfnWndProc = window_procedure;
    window_class.hInstance = instance;
    window_class.hCursor = LoadCursorW(NULL, IDC_ARROW);
    window_class.hbrBackground = (HBRUSH)(INT_PTR)(COLOR_WINDOW + 1);
    window_class.lpszClassName = eggpad_class;
    if (RegisterClassExW(&window_class) == 0) {
        printf("EGGPAD class failed\n");
        return 1;
    }
    window = CreateWindowExW(0, eggpad_class, L"eggpad", WS_OVERLAPPEDWINDOW, CW_USEDEFAULT,
                             CW_USEDEFAULT, EGGPAD_CLIENT_WIDTH + EGGPAD_BORDERS,
                             EGGPAD_CLIENT_HEIGHT + EGGPAD_BORDERS + EGGPAD_TITLE_BAR, NULL,
                             NULL, instance, NULL);
    if (window == NULL) {
        printf("EGGPAD window failed\n");
        return 1;
    }
    ShowWindow(window, show);
    UpdateWindow(window);
    while (GetMessageW(&message, NULL, 0, 0) > 0) {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
    free(text);
    return (int)message.wParam;
}
