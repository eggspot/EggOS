#include <windows.h>
#include <stdio.h>
#include <wchar.h>

#define FOLDER L"C:\\tmp\\eggsync"

static volatile LONG noted;
static volatile DWORD sleeper_result;
static HANDLE sleeper_ready;
static DWORD last_find_error;

static void CALLBACK note(ULONG_PTR value)
{
    noted = (LONG)value;
}

static DWORD WINAPI sleeper(LPVOID parameter)
{
    (void)parameter;
    SetEvent(sleeper_ready);
    sleeper_result = SleepEx(INFINITE, TRUE);
    return 0;
}

static DWORD WINAPI taker(LPVOID parameter)
{
    return WaitForSingleObject((HANDLE)parameter, INFINITE);
}

static void end_thread(HANDLE thread)
{
    WaitForSingleObject(thread, INFINITE);
    CloseHandle(thread);
}

static void apcs(void)
{
    HANDLE thread;
    HANDLE event;
    DWORD slept;
    DWORD alerted;
    DWORD signaled;

    QueueUserAPC(note, GetCurrentThread(), 7);
    slept = SleepEx(0, TRUE);
    printf("EGGSYNC apc on self ran %ld, sleep %lu\n", noted, slept);

    sleeper_ready = CreateEventW(NULL, TRUE, FALSE, NULL);
    thread = CreateThread(NULL, 0, sleeper, NULL, 0, NULL);
    WaitForSingleObject(sleeper_ready, INFINITE);
    QueueUserAPC(note, thread, 11);
    end_thread(thread);
    printf("EGGSYNC apc woke a sleeping thread with %ld, its sleep %lu\n", noted, sleeper_result);

    event = CreateEventW(NULL, TRUE, FALSE, NULL);
    QueueUserAPC(note, GetCurrentThread(), 13);
    alerted = WaitForSingleObjectEx(event, INFINITE, TRUE);
    SetEvent(event);
    signaled = WaitForSingleObjectEx(event, 0, TRUE);
    printf("EGGSYNC alertable wait %lu after %ld, then the event %lu\n", alerted, noted,
           signaled);
    CloseHandle(event);
    CloseHandle(sleeper_ready);
}

static HANDLE abandoned_mutex(void)
{
    HANDLE mutex = CreateMutexW(NULL, FALSE, NULL);
    end_thread(CreateThread(NULL, 0, taker, mutex, 0, NULL));
    return mutex;
}

static void mutexes(void)
{
    HANDLE mutex = abandoned_mutex();
    HANDLE pair[2];
    DWORD abandoned = WaitForSingleObject(mutex, 1000);
    BOOL released = ReleaseMutex(mutex);
    BOOL again = ReleaseMutex(mutex);
    DWORD stranger = GetLastError();
    DWORD any;

    printf("EGGSYNC abandoned %lu, owner released %d, then %d with %lu\n", abandoned, released,
           again, stranger);
    pair[0] = CreateEventW(NULL, TRUE, FALSE, NULL);
    pair[1] = abandoned_mutex();
    any = WaitForMultipleObjects(2, pair, FALSE, 1000);
    printf("EGGSYNC wait any abandoned %lu\n", any);
    ReleaseMutex(pair[1]);
    CloseHandle(pair[0]);
    CloseHandle(pair[1]);
    CloseHandle(mutex);
}

static void message_waits(void)
{
    HANDLE event = CreateEventW(NULL, TRUE, TRUE, NULL);
    DWORD object = MsgWaitForMultipleObjectsEx(1, &event, INFINITE, QS_ALLINPUT, 0);
    DWORD timeout = MsgWaitForMultipleObjects(0, NULL, FALSE, 10, QS_ALLINPUT);
    DWORD alerted;

    QueueUserAPC(note, GetCurrentThread(), 17);
    alerted = MsgWaitForMultipleObjectsEx(0, NULL, INFINITE, QS_ALLINPUT, MWMO_ALERTABLE);
    printf("EGGSYNC message waits %lu, %lu, alertable %lu after %ld\n", object, timeout, alerted,
           noted);
    CloseHandle(event);
}

static int same(HMODULE library, int ordinal, const char *name)
{
    FARPROC by_ordinal = GetProcAddress(library, MAKEINTRESOURCEA(ordinal));
    return by_ordinal != NULL && by_ordinal == GetProcAddress(library, name);
}

static void ordinals(void)
{
    HMODULE ws2_32 = LoadLibraryW(L"ws2_32.dll");
    HMODULE wsock32 = LoadLibraryW(L"wsock32.dll");
    FARPROC missing = GetProcAddress(ws2_32, MAKEINTRESOURCEA(9999));
    DWORD missing_error = GetLastError();

    printf("EGGSYNC ordinals ws2_32 %d %d, wsock32 %d, missing %d %lu\n",
           same(ws2_32, 115, "WSAStartup"), same(ws2_32, 10, "ioctlsocket"),
           same(wsock32, 12, "ioctlsocket"), missing == NULL, missing_error);
}

static void make_file(const wchar_t *name)
{
    wchar_t path[MAX_PATH];
    HANDLE file;

    wcscpy(path, FOLDER L"\\");
    wcscat(path, name);
    file = CreateFileW(path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    CloseHandle(file);
}

static int count(const wchar_t *pattern)
{
    wchar_t path[MAX_PATH];
    WIN32_FIND_DATAW found;
    HANDLE search;
    int matched = 0;

    wcscpy(path, FOLDER L"\\");
    wcscat(path, pattern);
    search = FindFirstFileW(path, &found);
    if (search == INVALID_HANDLE_VALUE) {
        return -(int)GetLastError();
    }
    do {
        if (found.cFileName[0] != L'.') {
            matched++;
        }
    } while (FindNextFileW(search, &found));
    last_find_error = GetLastError();
    FindClose(search);
    return matched;
}

static void wildcards(void)
{
    int none;

    CreateDirectoryW(FOLDER, NULL);
    make_file(L"a.txt");
    make_file(L"b.txt");
    make_file(L"readme");
    make_file(L"ab.c");
    printf("EGGSYNC found *.txt %d, *. %d, ??.c %d, <.c %d, A?.TXT %d\n", count(L"*.txt"),
           count(L"*."), count(L"??.c"), count(L"<.c"), count(L"A?.TXT"));
    none = count(L"x*");
    printf("EGGSYNC no match %d, end %lu\n", none, last_find_error);
}

int main(void)
{
    apcs();
    mutexes();
    message_waits();
    ordinals();
    wildcards();
    return 0;
}
