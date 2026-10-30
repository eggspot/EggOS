#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define ENDING_STATUS "3"

static volatile sig_atomic_t interrupted;

static int ascending(const void *a, const void *b)
{
    int left = *(const int *)a;
    int right = *(const int *)b;
    return (left > right) - (left < right);
}

static void goodbye(void)
{
    printf("EGGMINGW atexit ran\n");
}

static void on_interrupt(int number)
{
    interrupted = number;
}

int main(int argc, char **argv)
{
    int numbers[] = {42, 7, 19, 3, 25};
    char formatted[64];
    long long big = 1234567890123LL;
    const char *greeting = getenv("GREETING");
    char *text;
    int index;

    atexit(goodbye);
    printf("EGGMINGW hello from msvcrt.dll, arguments %d:", argc - 1);
    for (index = 1; index < argc; index++) {
        printf("%s%s", index == 1 ? " " : "|", argv[index]);
    }
    printf("\n");
    printf("EGGMINGW greeting %s\n", greeting != NULL ? greeting : "missing");

    qsort(numbers, sizeof numbers / sizeof numbers[0], sizeof numbers[0], ascending);
    printf("EGGMINGW sorted %d %d %d %d %d\n", numbers[0], numbers[1], numbers[2], numbers[3],
           numbers[4]);

    text = malloc(8);
    strcpy(text, "heap");
    text = realloc(text, 32);
    strcat(text, " grown");
    printf("EGGMINGW %s to %d bytes\n", text, (int)strlen(text));
    free(text);

    _snprintf(formatted, sizeof formatted, "%I64d|%-4s|%5.2f", big, "ms", 2.5);
    printf("EGGMINGW msvcrt formatted %s, counted %d\n", formatted,
           _scprintf("%s-%d", "abc", 12345));

    signal(SIGINT, on_interrupt);
    raise(SIGINT);
    printf("EGGMINGW signal %d\n", (int)interrupted);
    printf("EGGMINGW strerror %s\n", strerror(ENOENT));
    fprintf(stderr, "EGGMINGW stderr %ld\n", strtol("-77", NULL, 10));
    fflush(stdout);
    return atoi(ENDING_STATUS);
}
