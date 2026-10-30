#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>

#define FOLDER L"C:\\tmp\\eggnamed"
#define MAPPING_SIZE 65536
#define ARGUMENTS_AT 256
#define SAW_AT 1024
#define SIGNALED_AT 2048

static int child(int argc, char **argv)
{
    HANDLE mapping;
    HANDLE mutex;
    HANDLE event;
    HANDLE semaphore;
    char *view;
    LONG previous = -1;
    DWORD owned;
    DWORD signaled;
    int at;
    int index;

    mapping = OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE, L"Local\\EggNamedMapping");
    if (mapping == NULL) {
        return 10;
    }
    view = (char *)MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, 0);
    if (view == NULL) {
        return 11;
    }
    at = sprintf(view + ARGUMENTS_AT, "args %d:", argc - 1);
    for (index = 1; index < argc; index++) {
        at += sprintf(view + ARGUMENTS_AT + at, " %s", argv[index]);
    }
    mutex = OpenMutexW(SYNCHRONIZE, FALSE, L"EggNamedMutex");
    event = OpenEventW(EVENT_MODIFY_STATE, FALSE, L"EggNamedEvent");
    semaphore = OpenSemaphoreW(SEMAPHORE_MODIFY_STATE, FALSE, L"Global\\EggNamedSemaphore");
    if (mutex == NULL || event == NULL || semaphore == NULL) {
        return 12;
    }
    owned = WaitForSingleObject(mutex, 0);
    ReleaseSemaphore(semaphore, 2, &previous);
    sprintf(view + SAW_AT, "saw %s, mutex %lu, semaphore before %ld", view, owned, previous);
    signaled = SignalObjectAndWait(event, mutex, 10000, FALSE);
    sprintf(view + SIGNALED_AT, "signal and wait %lu", signaled);
    UnmapViewOfFile(view);
    return 0;
}

static HANDLE make_file(const wchar_t *path, const char *text)
{
    HANDLE file;
    DWORD written = 0;

    file = CreateFileW(path, GENERIC_READ | GENERIC_WRITE, 0, NULL, CREATE_ALWAYS,
                       FILE_ATTRIBUTE_NORMAL, NULL);
    if (file != INVALID_HANDLE_VALUE) {
        WriteFile(file, text, (DWORD)strlen(text), &written, NULL);
    }
    return file;
}

static void names(void)
{
    HANDLE mutex;
    HANDLE again;
    HANDLE other;
    DWORD created;
    DWORD existed;
    DWORD as_event;
    DWORD missing;
    DWORD bad_path;

    mutex = CreateMutexW(NULL, FALSE, L"EggNamedProbe");
    created = GetLastError();
    again = CreateMutexA(NULL, TRUE, "Local\\EggNamedProbe");
    existed = GetLastError();
    other = CreateEventW(NULL, FALSE, FALSE, L"EggNamedProbe");
    as_event = other == NULL ? GetLastError() : 0;
    other = OpenEventW(SYNCHRONIZE, FALSE, L"EggNamedMissing");
    missing = other == NULL ? GetLastError() : 0;
    other = CreateSemaphoreW(NULL, 0, 1, L"Other\\EggNamedProbe");
    bad_path = other == NULL ? GetLastError() : 0;
    printf("EGGNAMED names created %lu, again %lu with a new handle %d, as an event %lu, "
           "missing %lu, "
           "bad path %lu\n",
           created, existed, again != mutex && again != NULL, as_event, missing, bad_path);
    CloseHandle(again);
    CloseHandle(mutex);
}

static void processes(const wchar_t *self)
{
    HANDLE mutex;
    HANDLE event;
    HANDLE semaphore;
    HANDLE mapping;
    char *view;
    wchar_t command_line[MAX_PATH + 64];
    STARTUPINFOW startup;
    PROCESS_INFORMATION started;
    DWORD signaled;
    DWORD released;
    DWORD ended;
    DWORD code = 99;
    DWORD first;
    DWORD second;
    DWORD third;
    DWORD abandoned;
    DWORD gone;

    mutex = CreateMutexW(NULL, TRUE, L"EggNamedMutex");
    event = CreateEventW(NULL, FALSE, FALSE, L"EggNamedEvent");
    semaphore = CreateSemaphoreW(NULL, 0, 5, L"Global\\EggNamedSemaphore");
    mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, MAPPING_SIZE,
                                 L"EggNamedMapping");
    view = mapping == NULL ? NULL : (char *)MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, 0);
    if (mutex == NULL || event == NULL || semaphore == NULL || view == NULL) {
        printf("EGGNAMED named objects failed %lu\n", GetLastError());
        return;
    }
    strcpy(view, "the parent");
    swprintf(command_line, MAX_PATH + 64, L"\"%ls\" child %ls\\*.txt", self, FOLDER);
    memset(&startup, 0, sizeof startup);
    startup.cb = (DWORD)sizeof startup;
    if (!CreateProcessW(NULL, command_line, NULL, NULL, FALSE, 0, NULL, NULL, &startup,
                        &started)) {
        printf("EGGNAMED CreateProcessW failed %lu\n", GetLastError());
        return;
    }
    signaled = WaitForSingleObject(event, 10000);
    released = ReleaseMutex(mutex);
    ended = WaitForSingleObject(started.hProcess, 10000);
    GetExitCodeProcess(started.hProcess, &code);
    CloseHandle(started.hThread);
    CloseHandle(started.hProcess);
    printf("EGGNAMED child signalled %lu, released %lu, ended %lu with %lu\n", signaled,
           released, ended, code);
    printf("EGGNAMED child %s\n", view + ARGUMENTS_AT);
    printf("EGGNAMED child %s\n", view + SAW_AT);
    printf("EGGNAMED child %s\n", view + SIGNALED_AT);
    first = WaitForSingleObject(semaphore, 0);
    second = WaitForSingleObject(semaphore, 0);
    third = WaitForSingleObject(semaphore, 0);
    abandoned = WaitForSingleObject(mutex, 0);
    released = ReleaseMutex(mutex);
    printf("EGGNAMED semaphore %lu %lu %lu, mutex after the child ended %lu, released %lu\n",
           first, second, third, abandoned, released);
    UnmapViewOfFile(view);
    CloseHandle(mapping);
    mapping = OpenFileMappingW(FILE_MAP_READ, FALSE, L"EggNamedMapping");
    gone = mapping == NULL ? GetLastError() : 0;
    printf("EGGNAMED mapping gone after the last handle and view %lu\n", gone);
    CloseHandle(semaphore);
    CloseHandle(event);
    CloseHandle(mutex);
}

static void page_file(void)
{
    HANDLE mapping;
    HANDLE empty;
    char *writer;
    char *reader;
    char *misaligned;
    char *too_long;
    DWORD empty_error;
    DWORD misaligned_error;
    DWORD too_long_error;
    int same;

    mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, 8192, NULL);
    writer = (char *)MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, 0);
    reader = (char *)MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 4096);
    if (writer == NULL || reader == NULL) {
        printf("EGGNAMED page file views failed %lu\n", GetLastError());
        return;
    }
    strcpy(writer + 100, "shared");
    same = strcmp(reader + 100, "shared") == 0 && writer != reader;
    empty = CreateFileMappingW(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, 0, NULL);
    empty_error = empty == NULL ? GetLastError() : 0;
    misaligned = (char *)MapViewOfFile(mapping, FILE_MAP_READ, 0, 4096, 0);
    misaligned_error = misaligned == NULL ? GetLastError() : 0;
    too_long = (char *)MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 9000);
    too_long_error = too_long == NULL ? GetLastError() : 0;
    printf("EGGNAMED page file views share %d, empty %lu, misaligned %lu, too long %lu\n",
           same, empty_error, misaligned_error, too_long_error);
    UnmapViewOfFile(reader);
    UnmapViewOfFile(writer);
    CloseHandle(mapping);
}

static void file_views(void)
{
    HANDLE file;
    HANDLE mapping;
    char *view;
    char read_back[8];
    char copied[8];
    DWORD got = 0;
    BOOL flushed;
    DWORD read_only_error;

    file = make_file(FOLDER L"\\c.log", "hello");
    mapping = CreateFileMappingW(file, NULL, PAGE_READWRITE, 0, 0, NULL);
    view = mapping == NULL ? NULL : (char *)MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, 0);
    if (view == NULL) {
        printf("EGGNAMED file view failed %lu\n", GetLastError());
        return;
    }
    view[0] = 'j';
    flushed = FlushViewOfFile(view, 0);
    UnmapViewOfFile(view);
    CloseHandle(mapping);

    mapping = CreateFileMappingW(file, NULL, PAGE_WRITECOPY, 0, 0, NULL);
    view = mapping == NULL ? NULL : (char *)MapViewOfFile(mapping, FILE_MAP_COPY, 0, 0, 0);
    memset(copied, 0, sizeof copied);
    if (view != NULL) {
        view[1] = 'u';
        memcpy(copied, view, 5);
        UnmapViewOfFile(view);
    }
    CloseHandle(mapping);

    mapping = CreateFileMappingW(file, NULL, PAGE_READONLY, 0, 0, NULL);
    view = (char *)MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, 0);
    read_only_error = view == NULL ? GetLastError() : 0;
    CloseHandle(mapping);

    memset(read_back, 0, sizeof read_back);
    SetFilePointer(file, 0, NULL, FILE_BEGIN);
    ReadFile(file, read_back, 5, &got, NULL);
    CloseHandle(file);
    printf("EGGNAMED file view wrote %s, flushed %d, copy %s, read-only write %lu\n",
           read_back, flushed, copied, read_only_error);
}

int main(int argc, char **argv)
{
    wchar_t self[MAX_PATH];
    HANDLE file;

    if (argc > 1 && strcmp(argv[1], "child") == 0) {
        return child(argc, argv);
    }
    if (!GetModuleFileNameW(NULL, self, MAX_PATH)) {
        printf("EGGNAMED GetModuleFileNameW failed %lu\n", GetLastError());
        return 1;
    }
    CreateDirectoryW(FOLDER, NULL);
    file = make_file(FOLDER L"\\a.txt", "a");
    CloseHandle(file);
    file = make_file(FOLDER L"\\b.txt", "b");
    CloseHandle(file);
    names();
    processes(self);
    page_file();
    file_views();
    printf("EGGNAMED done\n");
    return 0;
}
