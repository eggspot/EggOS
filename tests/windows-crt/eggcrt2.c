#include <windows.h>
#include <fcntl.h>
#include <io.h>
#include <locale.h>
#include <math.h>
#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <wchar.h>

#define FOLDER "C:\\tmp\\eggcrt2"

static volatile double one = 1.0;
static volatile double two = 2.0;
static volatile double half = 0.5;
static volatile double ten = 10.0;
static volatile double three = 3.0;
static volatile double five = 5.0;

static jmp_buf jump;
static volatile int finally_ran;

static void math(void)
{
    printf("EGGCRT2 sin %.12g exp %.12g pow %.12g\n", sin(one), exp(one), pow(two, half));
    printf("EGGCRT2 atan2 %.12g fmod %.17g tgamma %.12g sqrt %.17g\n", atan2(one, one),
           fmod(ten, three), tgamma(five), sqrt(two));
}

static __declspec(noinline) void deepest(int value)
{
    longjmp(jump, value);
}

static __declspec(noinline) void inner(int value)
{
    __try {
        deepest(value);
    } __finally {
        finally_ran++;
    }
}

static void jumps(void)
{
    volatile int round = 0;
    int value = setjmp(jump);
    round++;
    if (round == 1) {
        inner(5);
    }
    if (round == 2) {
        inner(0);
    }
    printf("EGGCRT2 longjmp returned %d after %d rounds, finally ran %d\n", value, round,
           finally_ran);
}

static void locales(void)
{
    char initial[64];
    char user[64];
    char mixed[256];
    struct lconv *conventions;
    int user_longest;
    _locale_t created;
    const char *refused;

    snprintf(initial, sizeof initial, "%s", setlocale(LC_ALL, NULL));
    snprintf(user, sizeof user, "%s", setlocale(LC_ALL, ""));
    conventions = localeconv();
    user_longest = MB_CUR_MAX;
    printf("EGGCRT2 locale %s, user %s, separators %s %s, grouping %d, currency %s, "
           "MB_CUR_MAX %d\n",
           initial, user, conventions->decimal_point, conventions->thousands_sep,
           conventions->grouping[0], conventions->currency_symbol, user_longest);
    setlocale(LC_NUMERIC, "C");
    snprintf(mixed, sizeof mixed, "%s", setlocale(LC_ALL, NULL));
    printf("EGGCRT2 locale numeric C: %d long, numeric part %d, starts %.38s\n",
           (int)strlen(mixed), strstr(mixed, ";LC_NUMERIC=C;") != NULL, mixed);
    refused = setlocale(LC_ALL, "de-DE");
    created = _create_locale(LC_ALL, "en-US");
    printf("EGGCRT2 locale de-DE refused %d, en-US object %d, ", refused == NULL,
           created != NULL);
    _free_locale(created);
    setlocale(LC_ALL, "C");
    printf("back to C %d\n", MB_CUR_MAX);
}

static int file_size(const char *path)
{
    unsigned char raw[64];
    int fd = _open(path, _O_RDONLY | _O_BINARY);
    int size = _read(fd, raw, (unsigned)sizeof raw);
    _close(fd);
    return size;
}

static void wide_streams(void)
{
    wchar_t line[16];
    unsigned char raw[8];
    FILE *file;
    wint_t first;
    wint_t again;
    wint_t last;
    int ended;
    int fd;
    int size;

    file = _wfopen(L"C:\\tmp\\eggcrt2\\wide.txt", L"w, ccs=UTF-16LE");
    fputws(L"h\x00e9llo\n", file);
    fputwc(L'!', file);
    fclose(file);
    file = _wfopen(L"C:\\tmp\\eggcrt2\\wide.txt", L"r, ccs=UNICODE");
    fgetws(line, 16, file);
    first = fgetwc(file);
    ungetwc(first, file);
    again = fgetwc(file);
    last = fgetwc(file);
    ended = feof(file) != 0;
    fclose(file);
    printf("EGGCRT2 wide file %d bytes, line length %d second %x, then %c %c end %d %d\n",
           file_size(FOLDER "\\wide.txt"), (int)wcslen(line), line[1], first, again,
           last == WEOF, ended);

    file = fopen(FOLDER "\\multibyte.txt", "w");
    fputws(L"\x00e9\n", file);
    fclose(file);
    fd = _open(FOLDER "\\multibyte.txt", _O_RDONLY | _O_BINARY);
    size = _read(fd, raw, (unsigned)sizeof raw);
    _close(fd);
    printf("EGGCRT2 multibyte file %d bytes %02x %02x %02x %02x\n", size, raw[0], raw[1],
           raw[2], raw[3]);
}

static void show_lines(char *text, int length)
{
    int index;
    for (index = 0; index < length; index++) {
        if (text[index] == '\n') {
            text[index] = '|';
        }
    }
    text[length] = 0;
}

static void text_mode(void)
{
    char text[32];
    int fd;
    int wrote;
    int got;
    int stopped;
    int after;
    int was;

    fd = _open(FOLDER "\\text.txt", _O_CREAT | _O_TRUNC | _O_WRONLY | _O_TEXT,
               _S_IREAD | _S_IWRITE);
    wrote = _write(fd, "a\nb\n", 4);
    _close(fd);
    fd = _open(FOLDER "\\text.txt", _O_RDONLY | _O_TEXT);
    got = _read(fd, text, (unsigned)sizeof text - 1);
    _close(fd);
    show_lines(text, got);
    printf("EGGCRT2 text wrote %d, file %d, read %d %s\n", wrote,
           file_size(FOLDER "\\text.txt"), got, text);

    fd = _open(FOLDER "\\ended.txt", _O_CREAT | _O_TRUNC | _O_WRONLY | _O_BINARY,
               _S_IREAD | _S_IWRITE);
    _write(fd, "x\r\ny\x1az", 6);
    _close(fd);
    fd = _open(FOLDER "\\ended.txt", _O_RDONLY | _O_TEXT);
    stopped = _read(fd, text, (unsigned)sizeof text - 1);
    after = _read(fd, text + stopped, (unsigned)(sizeof text - 1 - stopped));
    was = _setmode(fd, _O_BINARY);
    _close(fd);
    show_lines(text, stopped);
    printf("EGGCRT2 ctrl+z read %d %s then %d, setmode was %d\n", stopped, text, after, was);
}

int main(void)
{
    CreateDirectoryW(L"C:\\tmp\\eggcrt2", NULL);
    math();
    jumps();
    locales();
    wide_streams();
    text_mode();
    printf("EGGCRT2 done\n");
    return 0;
}
