#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHILD_EXIT 42
#define TERMINATED_EXIT 9
#define PEEKED 8
#define DIRECTORY_BYTES 512

static wchar_t self[MAX_PATH];

static int child(int argc, char **argv)
{
    char greeting[32] = "";
    wchar_t wide_directory[MAX_PATH] = L"";
    char directory[DIRECTORY_BYTES] = "";
    int index;

    printf("args %d:", argc - 1);
    for (index = 1; index < argc; index++) {
        printf("%s%s", index == 1 ? " " : "|", argv[index]);
    }
    printf("\n");
    GetEnvironmentVariableA("EGGSPAWN_GREETING", greeting, (DWORD)sizeof greeting);
    printf("greeting %s\n", greeting);
    GetCurrentDirectoryW(MAX_PATH, wide_directory);
    WideCharToMultiByte(CP_UTF8, 0, wide_directory, -1, directory, DIRECTORY_BYTES, NULL, NULL);
    printf("directory %s\n", directory);
    return CHILD_EXIT;
}

static wchar_t *environment_with_greeting(void)
{
    const wchar_t *greeting = L"EGGSPAWN_GREETING=hello";
    wchar_t *inherited = GetEnvironmentStringsW();
    size_t inherited_units = 0;
    size_t greeting_units = wcslen(greeting) + 1;
    wchar_t *block;

    while (inherited[inherited_units] != 0) {
        inherited_units += wcslen(inherited + inherited_units) + 1;
    }
    block = malloc((inherited_units + greeting_units + 1) * sizeof(wchar_t));
    if (block != NULL) {
        memcpy(block, inherited, inherited_units * sizeof(wchar_t));
        memcpy(block + inherited_units, greeting, greeting_units * sizeof(wchar_t));
        block[inherited_units + greeting_units] = 0;
    }
    FreeEnvironmentStringsW(inherited);
    return block;
}

static void print_lines(char *output, DWORD length)
{
    DWORD start = 0;
    DWORD index;

    for (index = 0; index < length; index++) {
        if (output[index] == '\n') {
            output[index] = 0;
            if (index > start && output[index - 1] == '\r') {
                output[index - 1] = 0;
            }
            printf("EGGSPAWN child said %s\n", output + start);
            start = index + 1;
        }
    }
}

static int through_a_pipe(void)
{
    SECURITY_ATTRIBUTES inheritable = {(DWORD)sizeof(SECURITY_ATTRIBUTES), NULL, TRUE};
    HANDLE read_end;
    HANDLE write_end;
    HANDLE duplicate;
    HANDLE process_duplicate;
    DWORD read_flags = 99;
    DWORD write_flags = 99;
    DWORD duplicate_flags = 99;
    STARTUPINFOW startup;
    PROCESS_INFORMATION started;
    wchar_t command_line[MAX_PATH + 64];
    wchar_t *environment = environment_with_greeting();
    char output[4096];
    char first[PEEKED];
    DWORD total = 0;
    DWORD got = 0;
    DWORD peeked = 0;
    DWORD available = 0;
    DWORD code = 0;
    DWORD duplicate_code = 0;
    BOOL refused;
    BOOL closed;
    int broken;

    if (environment == NULL || !CreatePipe(&read_end, &write_end, &inheritable, 0)) {
        printf("EGGSPAWN CreatePipe failed %lu\n", GetLastError());
        return 0;
    }
    SetHandleInformation(read_end, HANDLE_FLAG_INHERIT, 0);
    GetHandleInformation(read_end, &read_flags);
    GetHandleInformation(write_end, &write_flags);
    printf("EGGSPAWN pipe inherit read %lu write %lu\n", read_flags, write_flags);

    wcscpy(command_line, L"\"");
    wcscat(command_line, self);
    wcscat(command_line, L"\" child \"two words\" back\\slash \"q\\\"uote\"");
    memset(&startup, 0, sizeof startup);
    startup.cb = (DWORD)sizeof startup;
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    startup.hStdOutput = write_end;
    startup.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    if (!CreateProcessW(self, command_line, NULL, NULL, TRUE,
                        CREATE_UNICODE_ENVIRONMENT | CREATE_NEW_CONSOLE, environment, L"C:\\tmp",
                        &startup, &started)) {
        printf("EGGSPAWN CreateProcessW failed %lu\n", GetLastError());
        return 0;
    }
    free(environment);
    CloseHandle(write_end);
    if (WaitForSingleObject(started.hProcess, 10000) != WAIT_OBJECT_0) {
        printf("EGGSPAWN the child did not end\n");
        return 0;
    }
    PeekNamedPipe(read_end, first, PEEKED, &peeked, &available, NULL);
    while (ReadFile(read_end, output + total, (DWORD)(sizeof output - 1 - total), &got, NULL) &&
           got != 0) {
        total += got;
    }
    broken = GetLastError() == ERROR_BROKEN_PIPE;
    printf("EGGSPAWN peeked %lu, all %d read, same %d, then broken pipe %d\n", peeked,
           available == total, memcmp(first, output, PEEKED) == 0, broken);
    print_lines(output, total);

    GetExitCodeProcess(started.hProcess, &code);
    DuplicateHandle(GetCurrentProcess(), started.hProcess, GetCurrentProcess(), &process_duplicate,
                    0, FALSE, DUPLICATE_SAME_ACCESS);
    GetExitCodeProcess(process_duplicate, &duplicate_code);
    printf("EGGSPAWN child exit %lu, through a duplicate %lu\n", code, duplicate_code);
    CloseHandle(process_duplicate);
    CloseHandle(started.hThread);
    CloseHandle(started.hProcess);

    DuplicateHandle(GetCurrentProcess(), read_end, GetCurrentProcess(), &duplicate, 0, TRUE,
                    DUPLICATE_SAME_ACCESS);
    GetHandleInformation(duplicate, &duplicate_flags);
    SetHandleInformation(duplicate, HANDLE_FLAG_PROTECT_FROM_CLOSE, HANDLE_FLAG_PROTECT_FROM_CLOSE);
    refused = !CloseHandle(duplicate);
    SetHandleInformation(duplicate, HANDLE_FLAG_PROTECT_FROM_CLOSE, 0);
    closed = CloseHandle(duplicate);
    printf("EGGSPAWN duplicate inherit %lu, protected close refused %d then closed %d\n",
           duplicate_flags, refused, closed);
    CloseHandle(read_end);
    return 1;
}

static int terminated(void)
{
    STARTUPINFOW startup;
    PROCESS_INFORMATION started;
    wchar_t command_line[MAX_PATH + 16];
    DWORD running = 0;
    DWORD code = 0;
    int timed_out;

    wcscpy(command_line, L"\"");
    wcscat(command_line, self);
    wcscat(command_line, L"\" wait");
    memset(&startup, 0, sizeof startup);
    startup.cb = (DWORD)sizeof startup;
    if (!CreateProcessW(NULL, command_line, NULL, NULL, FALSE, 0, NULL, NULL, &startup,
                        &started)) {
        printf("EGGSPAWN CreateProcessW failed %lu\n", GetLastError());
        return 0;
    }
    GetExitCodeProcess(started.hProcess, &running);
    timed_out = WaitForSingleObject(started.hProcess, 50) == WAIT_TIMEOUT;
    TerminateProcess(started.hProcess, TERMINATED_EXIT);
    WaitForSingleObject(started.hProcess, INFINITE);
    GetExitCodeProcess(started.hProcess, &code);
    printf("EGGSPAWN terminate: running %lu, timeout %d, exit %lu\n", running, timed_out, code);
    CloseHandle(started.hThread);
    CloseHandle(started.hProcess);
    return 1;
}

static int ansi_and_suspended(void)
{
    STARTUPINFOA startup;
    PROCESS_INFORMATION started;
    char command_line[] = "eggspawn.exe exit 5";
    DWORD first;
    DWORD second;
    DWORD code = 0;

    memset(&startup, 0, sizeof startup);
    startup.cb = (DWORD)sizeof startup;
    if (!CreateProcessA(NULL, command_line, NULL, NULL, FALSE, CREATE_SUSPENDED, NULL, NULL,
                        &startup, &started)) {
        printf("EGGSPAWN CreateProcessA failed %lu\n", GetLastError());
        return 0;
    }
    first = ResumeThread(started.hThread);
    second = ResumeThread(started.hThread);
    WaitForSingleObject(started.hProcess, INFINITE);
    GetExitCodeProcess(started.hProcess, &code);
    printf("EGGSPAWN ansi suspended resumed %lu then %lu, exit %lu\n", first, second, code);
    CloseHandle(started.hThread);
    CloseHandle(started.hProcess);
    return 1;
}

static void missing(void)
{
    STARTUPINFOW startup;
    PROCESS_INFORMATION started;
    wchar_t command_line[] = L"no-such-program.exe";
    BOOL ok;

    memset(&startup, 0, sizeof startup);
    startup.cb = (DWORD)sizeof startup;
    ok = CreateProcessW(NULL, command_line, NULL, NULL, FALSE, 0, NULL, NULL, &startup, &started);
    printf("EGGSPAWN missing program started %d error %lu\n", ok, GetLastError());
}

int main(int argc, char **argv)
{
    if (argc >= 2 && strcmp(argv[1], "child") == 0) {
        return child(argc, argv);
    }
    if (argc >= 2 && strcmp(argv[1], "wait") == 0) {
        Sleep(60000);
        return 0;
    }
    if (argc >= 3 && strcmp(argv[1], "exit") == 0) {
        return atoi(argv[2]);
    }
    if (!GetModuleFileNameW(NULL, self, MAX_PATH)) {
        printf("EGGSPAWN GetModuleFileNameW failed %lu\n", GetLastError());
        return 1;
    }
    if (!through_a_pipe() || !terminated() || !ansi_and_suspended()) {
        return 1;
    }
    missing();
    printf("EGGSPAWN done\n");
    return 0;
}
