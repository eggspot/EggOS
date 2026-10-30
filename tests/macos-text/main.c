#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

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

int main(void) {
    printf("MACOS-TEXT main\n");
    numbers();
    floats();
    strings();
    endings();
    stream();
    spans();
    printf("MACOS-TEXT done\n");
    return 0;
}
