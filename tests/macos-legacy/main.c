#include <getopt.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static const struct option long_options[] = {
    {"name", required_argument, NULL, 'n'},
    {"verbose", no_argument, NULL, 'v'},
    {"vex", no_argument, NULL, 'V'},
    {"level", optional_argument, NULL, 'l'},
    {NULL, 0, NULL, 0},
};

static void restart(void) {
    optreset = 1;
    optind = 1;
}

static void print_words(const char *label, int count, char **words) {
    printf("%s", label);
    for (int index = 1; index < count; index++) {
        printf("%s%s", index == 1 ? "" : ",", words[index]);
    }
    printf("\n");
}

static void program_arguments(int argc, char **argv) {
    int verbose = 0;
    const char *name = "-";
    int option;
    while ((option = getopt_long(argc, argv, "vn:", long_options, NULL)) != -1) {
        if (option == 'v') {
            verbose++;
        } else if (option == 'n') {
            name = optarg;
        }
    }
    printf("MACOS-LEGACY argv v=%d name=%s optind=%d ", verbose, name, optind);
    print_words("words=", argc, argv);
}

static void permuted(void) {
    char *words[] = {"probe", "file1", "-v", "--name=x", "file2", "-n", "y", "--", "-v", "z", NULL};
    int count = 10;
    int option;
    int seen[8] = {0};
    int found = 0;
    restart();
    while ((option = getopt_long(count, words, "vn:l::", long_options, NULL)) != -1) {
        if (found < 8) {
            seen[found++] = optind;
        }
    }
    printf("MACOS-LEGACY permute optind=%d after=%d,%d,%d ", optind, seen[0], seen[1], seen[2]);
    print_words("order=", count, words);
}

static void in_order(void) {
    char *words[] = {"probe", "a", "-v", "b", NULL};
    int option;
    restart();
    printf("MACOS-LEGACY in-order");
    while ((option = getopt_long(4, words, "-vn:", long_options, NULL)) != -1) {
        if (option == 1) {
            printf(" 1:%s", optarg);
        } else {
            printf(" %c", option);
        }
    }
    printf(" optind=%d\n", optind);
}

static void posix(void) {
    char *words[] = {"probe", "a", "-v", NULL};
    restart();
    setenv("POSIXLY_CORRECT", "1", 1);
    int stopped = getopt_long(3, words, "v", long_options, NULL);
    int stopped_at = optind;
    unsetenv("POSIXLY_CORRECT");
    restart();
    int plus = getopt_long(3, words, "+v", long_options, NULL);
    printf("MACOS-LEGACY posix %d %d plus %d %d %s\n", stopped, stopped_at, plus, optind,
           words[1]);
}

static void long_only(void) {
    char *words[] = {"probe", "-verbose", "-name=q", "-nx", "-ve", "-lev", "a", NULL};
    int option;
    int index = -1;
    restart();
    opterr = 0;
    printf("MACOS-LEGACY long-only");
    while ((option = getopt_long_only(7, words, "vn:l::", long_options, &index)) != -1) {
        if (option == '?') {
            printf(" ?");
        } else if (optarg != NULL) {
            printf(" %c=%s", option, optarg);
        } else {
            printf(" %c#%d", option, index);
        }
    }
    opterr = 1;
    printf(" optind=%d\n", optind);
}

static void messages(void) {
    char *unknown[] = {"probe", "-x", "--bogus", "--verbose=1", "--name", NULL};
    restart();
    while (getopt_long(5, unknown, "vn:", long_options, NULL) != -1) {
    }
    char *plain[] = {"probe", "a", "-v", NULL};
    restart();
    int stopped = getopt(3, plain, "v");
    int stopped_at = optind;
    optind = 2;
    int found = getopt(3, plain, "v");
    printf("MACOS-LEGACY getopt %d %d then %c %d\n", stopped, stopped_at, found, optind);
}

static int compare(const void *left, const void *right) {
    return *(const int *)left - *(const int *)right;
}

static size_t (*const measure)(const char *) = strlen;
static const char *const greetings[] = {"hello", "world"};
static int (*const steps[])(const void *, const void *) = {compare, compare};

static void lazy(void) {
    int numbers[] = {3, 1, 2};
    qsort(numbers, 3, sizeof numbers[0], steps[1]);
    printf("MACOS-LEGACY lazy %zu %d %d %d,%d,%d %s %s %zu\n", strlen("eggos"),
           strcmp("a", "a"), atoi("42"), numbers[0], numbers[1], numbers[2], greetings[0],
           greetings[1], measure(greetings[1]));
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    printf("MACOS-LEGACY main\n");
    program_arguments(argc, argv);
    permuted();
    in_order();
    posix();
    long_only();
    messages();
    lazy();
    printf("MACOS-LEGACY done\n");
    return 0;
}
