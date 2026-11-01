#include <ctype.h>
#include <errno.h>
#include <langinfo.h>
#include <locale.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <wchar.h>
#include <wctype.h>

static int scan_list(const char *text, const char *format, ...) {
    va_list arguments;
    va_start(arguments, format);
    int count = vsscanf(text, format, arguments);
    va_end(arguments);
    return count;
}

static void numbers(void) {
    int decimal = 0, octal = 0, hex = 0, any = 0;
    unsigned int positive = 0;
    long wide = 0;
    long long longest = 0;
    signed char tiny = 0;
    short half = 0;
    size_t size = 0;
    int count = sscanf("  -42 017 0x1F 0x10 7 -9000000000 123456789012 300 -2 99",
                       "%d %o %x %i %u %ld %lld %hhd %hd %zu", &decimal, &octal, &hex, &any,
                       &positive, &wide, &longest, &tiny, &half, &size);
    printf("MACOS-TEXT numbers %d %d %d %d %d %u %ld %lld %d %d %zu\n", count, decimal, octal,
           hex, any, positive, wide, longest, tiny, half, size);
}

static void floats(void) {
    float single = 0;
    double twice = 0;
    long double longer = 0;
    int count = sscanf("2.5 -1.25e2 0.125", "%f %lf %Lf", &single, &twice, &longer);
    printf("MACOS-TEXT floats %d %.3f %.3f %.3Lf\n", count, single, twice, longer);
}

static void strings(void) {
    char word[16] = {0}, letters[8] = {0}, set[16] = {0}, rest[16] = {0};
    int consumed = 0, skipped = 0;
    void *pointer = NULL;
    int count = sscanf("hello  xyz abc123:tail 0x1000", "%15s %2c%*c %[a-c1-3]:%n%15s %p",
                       word, letters, set, &consumed, rest, &pointer);
    printf("MACOS-TEXT strings %d '%s' '%s' '%s' %d '%s' %p\n", count, word, letters, set,
           consumed, rest, pointer);
    count = sscanf("12 34", "%*d %d", &skipped);
    printf("MACOS-TEXT suppressed %d %d\n", count, skipped);
}

static void endings(void) {
    int first = -1, second = -1;
    char after = 0;
    unsigned int value = 7;
    int empty = sscanf("", "%d", &first);
    int blank = sscanf("   ", "%d", &first);
    int mismatch = sscanf("12 x", "%d %d", &first, &second);
    int literal = sscanf("7-8", "%d-%d", &first, &second);
    int prefix = sscanf("0xg", "%x%c", &value, &after);
    int listed = scan_list("5 6", "%d %d", &first, &second);
    printf("MACOS-TEXT endings %d %d %d %d %d %u '%c' %d %d %d\n", empty, blank, mismatch,
           literal, prefix, value, after, listed, first, second);
}

static void stream(void) {
    int ends[2];
    if (pipe(ends) != 0) {
        printf("MACOS-TEXT stream pipe failed\n");
        return;
    }
    const char content[] = "10 20\nname: eggos\n1e 3.5\n";
    if (write(ends[1], content, sizeof content - 1) != (ssize_t)(sizeof content - 1)) {
        printf("MACOS-TEXT stream write failed\n");
    }
    close(ends[1]);
    FILE *file = fdopen(ends[0], "r");
    int a = 0, b = 0;
    char name[16] = {0};
    float exponentless = 0, next = 0;
    int first = fscanf(file, "%d %d", &a, &b);
    int second = fscanf(file, " name: %15s", name);
    int third = fscanf(file, "%f", &exponentless);
    int letter = fgetc(file);
    int fourth = fscanf(file, "%f", &next);
    int end = fscanf(file, "%d", &a);
    printf("MACOS-TEXT stream %d %d %d %d '%s' %d %.1f '%c' %d %.1f %d %d\n", first, a, b, second,
           name, third, exponentless, letter, fourth, next, end, feof(file) != 0);
    fclose(file);
}

static void spans(void) {
    const char *text = "key=value; other";
    printf("MACOS-TEXT spans %zu %zu '%s' %d\n", strspn(text, "abcdefghijklmnopqrstuvwxyz"),
           strcspn(text, ";="), strpbrk(text, ";="), strpbrk(text, "!?") == NULL);
}

static void locale_names(void) {
    char initial[16];
    snprintf(initial, sizeof initial, "%s", setlocale(LC_ALL, NULL));
    const char *posix = setlocale(LC_ALL, "POSIX");
    printf("MACOS-TEXT locale %s %s %d\n", initial, posix, (int)MB_CUR_MAX);
    setlocale(LC_ALL, "C");
    setlocale(LC_CTYPE, "UTF-8");
    char mixed[96];
    snprintf(mixed, sizeof mixed, "%s", setlocale(LC_ALL, NULL));
    int refused = setlocale(LC_ALL, "UTF-8") == NULL;
    int unknown = setlocale(LC_ALL, "xx_YY.UTF-8") == NULL;
    int multibyte = (int)MB_CUR_MAX;
    setlocale(LC_ALL, "C.UTF-8");
    const char *restored = setlocale(LC_ALL, mixed);
    printf("MACOS-TEXT names %s %d %d %d %s\n", mixed, refused, unknown, multibyte,
           restored == NULL ? "(null)" : restored);
    unsetenv("LC_ALL");
    unsetenv("LC_CTYPE");
    setenv("LANG", "en_US.UTF-8", 1);
    setenv("LC_TIME", "C", 1);
    const char *environment = setlocale(LC_ALL, "");
    printf("MACOS-TEXT environment %s\n", environment == NULL ? "(null)" : environment);
    unsetenv("LC_TIME");
    unsetenv("LANG");
}

static void united_states(void) {
    setlocale(LC_ALL, "en_US.UTF-8");
    struct lconv *conventions = localeconv();
    printf("MACOS-TEXT en_US %s %s %s '%s' '%s' '%s' %d %d %s %s\n", nl_langinfo(CODESET),
           nl_langinfo(D_FMT), nl_langinfo(D_T_FMT), conventions->thousands_sep,
           conventions->int_curr_symbol, conventions->currency_symbol, conventions->grouping[0],
           conventions->frac_digits, nl_langinfo(YESSTR), nl_langinfo(CRNCYSTR));
    struct tm when;
    memset(&when, 0, sizeof when);
    when.tm_year = 126;
    when.tm_mon = 2;
    when.tm_mday = 5;
    when.tm_hour = 14;
    when.tm_min = 7;
    when.tm_sec = 9;
    when.tm_wday = 4;
    when.tm_yday = 63;
    char date[64];
    strftime(date, sizeof date, "%x|%c|%D", &when);
    struct tm parsed;
    memset(&parsed, 0, sizeof parsed);
    const char *end = strptime("03/05/2026 02:07:09 PM rest", "%x %r", &parsed);
    printf("MACOS-TEXT time '%s' %d %d %d %d %d %d %d %d '%s'\n", date, parsed.tm_year,
           parsed.tm_mon, parsed.tm_mday, parsed.tm_hour, parsed.tm_min, parsed.tm_sec,
           parsed.tm_wday, parsed.tm_yday, end == NULL ? "(null)" : end);
    memset(&parsed, 0, sizeof parsed);
    end = strptime("thursday, MARCH 5 26", "%A, %B %e %y", &parsed);
    int failed = strptime("24:00", "%H:%M", &parsed) == NULL;
    printf("MACOS-TEXT parsed %d %d %d %d %d %d '%s'\n", parsed.tm_year, parsed.tm_mon,
           parsed.tm_mday, parsed.tm_wday, parsed.tm_yday, failed, end == NULL ? "(null)" : end);
    memset(&parsed, 0, sizeof parsed);
    end = strptime("2026 64", "%Y %j", &parsed);
    printf("MACOS-TEXT year day %d %d %d %d %d\n", end != NULL, parsed.tm_mon, parsed.tm_mday,
           parsed.tm_wday, parsed.tm_yday);
}

static void characters(void) {
    printf("MACOS-TEXT ctype %d %d %d %d %d %X %X %X %X %X %d\n", isalpha(0xE9) != 0,
           isupper(0xC9) != 0, iswspace(0xA0) != 0, iswpunct(0xD7) != 0, isdigit('7') != 0,
           toupper(0xE9), tolower(0xC9), (unsigned)towupper(0xFF), (unsigned)towupper(0xB5),
           (unsigned)towlower(0x178), iswalpha(0xD7) != 0);
    wchar_t wide[8] = {0};
    size_t converted = mbstowcs(wide, "h\xC3\xA9\xE2\x82\xAC", 8);
    char narrow[16] = {0};
    size_t back = wcstombs(narrow, wide, sizeof narrow);
    mbstate_t state;
    memset(&state, 0, sizeof state);
    wchar_t one = 0;
    size_t first = mbrtowc(&one, "\xF0\x9F", 2, &state);
    size_t second = mbrtowc(&one, "\x98\x80", 2, &state);
    char bytes[8] = {0};
    size_t written = wcrtomb(bytes, 0x10FFFF, &state);
    wchar_t other = 0;
    errno = 0;
    size_t surrogate = mbrtowc(&other, "\xED\xA0\x80", 3, &state);
    int illegal = errno == EILSEQ;
    printf("MACOS-TEXT multibyte %zu %X %X %zu %d %zu %X %zu %02X %d %d %d %d\n", converted,
           (unsigned)wide[1], (unsigned)wide[2], back, first == (size_t)-2, second,
           (unsigned)one, written, (unsigned char)bytes[0], surrogate == (size_t)-1, illegal,
           mblen("\xC3\xA9", 2), strcmp(narrow, "h\xC3\xA9\xE2\x82\xAC") == 0);
    wchar_t word[8] = {0};
    int got = sscanf("h\xC3\xA9llo w", "%3ls", word);
    printf("MACOS-TEXT wide scan %d %X %X %X %X\n", got, (unsigned)word[0], (unsigned)word[1],
           (unsigned)word[2], (unsigned)word[3]);
    setlocale(LC_ALL, "C");
    one = 0;
    size_t single = mbrtowc(&one, "\xE9", 1, NULL);
    printf("MACOS-TEXT C again %X %d %d %zu %X %d\n", toupper(0xE9), isalpha(0xE9) != 0,
           (int)MB_CUR_MAX, single, (unsigned)one, wcrtomb(bytes, 0x100, NULL) == (size_t)-1);
}

int main(void) {
    printf("MACOS-TEXT main\n");
    numbers();
    floats();
    strings();
    endings();
    stream();
    spans();
    locale_names();
    united_states();
    characters();
    printf("MACOS-TEXT done\n");
    return 0;
}
