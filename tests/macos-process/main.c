#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

static const char *self_path = "/bin/macos-process";

static volatile sig_atomic_t caught_signal;
static volatile sig_atomic_t caught_code;
static volatile sig_atomic_t caught_from_self;
static volatile sig_atomic_t caught_count;
static volatile sig_atomic_t child_pid;
static volatile sig_atomic_t child_status;
static volatile sig_atomic_t child_code;
static char *alternate_base;
static volatile sig_atomic_t on_alternate;

static void on_info(int signal, siginfo_t *info, void *context) {
    (void)context;
    caught_signal = signal;
    caught_code = info->si_code;
    caught_from_self = info->si_pid == getpid();
}

static void on_plain(int signal) {
    caught_signal = signal;
    caught_count += 1;
}

static void on_alternate_stack(int signal) {
    char here;
    (void)signal;
    on_alternate = &here >= alternate_base && &here < alternate_base + SIGSTKSZ;
}

static void on_child(int signal, siginfo_t *info, void *context) {
    (void)signal;
    (void)context;
    child_pid = info->si_pid;
    child_status = info->si_status;
    child_code = info->si_code;
}

static void handlers(void) {
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_sigaction = on_info;
    action.sa_flags = SA_SIGINFO;
    sigemptyset(&action.sa_mask);
    struct sigaction previous;
    sigaction(SIGUSR1, &action, &previous);
    kill(getpid(), SIGUSR1);
    printf("MACOS-PROCESS siginfo %d %s %s default-before %d\n", caught_signal,
           caught_code == SI_USER ? "user" : "other", caught_from_self ? "self" : "other",
           previous.sa_handler == SIG_DFL);
    signal(SIGUSR2, on_plain);
    raise(SIGUSR2);
    printf("MACOS-PROCESS signal %d count %d\n", caught_signal, caught_count);
    sigset_t blocked;
    sigset_t pending;
    sigset_t empty;
    sigemptyset(&blocked);
    sigemptyset(&empty);
    sigaddset(&blocked, SIGUSR2);
    sigprocmask(SIG_BLOCK, &blocked, NULL);
    raise(SIGUSR2);
    int held = caught_count;
    sigpending(&pending);
    int suspended = sigsuspend(&empty);
    int suspend_errno = errno;
    sigprocmask(SIG_UNBLOCK, &blocked, NULL);
    printf("MACOS-PROCESS mask held %d pending %d suspend %d %s count %d\n", held,
           sigismember(&pending, SIGUSR2), suspended,
           suspend_errno == EINTR ? "EINTR" : "other", caught_count);
}

static void alternate_stack(void) {
    alternate_base = malloc(SIGSTKSZ);
    stack_t stack = {.ss_sp = alternate_base, .ss_size = SIGSTKSZ, .ss_flags = 0};
    stack_t old;
    int set = sigaltstack(&stack, &old);
    stack_t tiny = {.ss_sp = alternate_base, .ss_size = 1024, .ss_flags = 0};
    int refused = sigaltstack(&tiny, NULL);
    int refused_errno = errno;
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_handler = on_alternate_stack;
    action.sa_flags = SA_ONSTACK;
    sigaction(SIGUSR2, &action, NULL);
    raise(SIGUSR2);
    printf("MACOS-PROCESS altstack %d %s refused %d %s on %d\n", set,
           old.ss_flags & SS_DISABLE ? "was-disabled" : "was-set", refused,
           refused_errno == ENOMEM ? "ENOMEM" : "other", on_alternate);
    signal(SIGUSR2, SIG_DFL);
}

static void fork_and_pipe(void) {
    int ends[2];
    pipe(ends);
    fflush(stdout);
    pid_t child = fork();
    if (child == 0) {
        close(ends[0]);
        const char message[] = "hello from the child";
        write(ends[1], message, sizeof message - 1);
        _exit(7);
    }
    close(ends[1]);
    char buffer[64];
    ssize_t length = 0;
    ssize_t got;
    while ((got = read(ends[0], buffer + length, sizeof buffer - 1 - length)) > 0) {
        length += got;
    }
    buffer[length] = 0;
    close(ends[0]);
    int status = 0;
    pid_t waited = waitpid(child, &status, 0);
    printf("MACOS-PROCESS fork read '%s' same %d exited %d status %d\n", buffer, waited == child,
           WIFEXITED(status), WEXITSTATUS(status));
}

static void killed_child(void) {
    fflush(stdout);
    pid_t child = fork();
    if (child == 0) {
        for (;;) {
            pause();
        }
    }
    kill(child, SIGTERM);
    int status = 0;
    waitpid(child, &status, 0);
    printf("MACOS-PROCESS killed signaled %d signal %d\n", WIFSIGNALED(status), WTERMSIG(status));
}

static void child_signal(void) {
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_sigaction = on_child;
    action.sa_flags = SA_SIGINFO;
    sigaction(SIGCHLD, &action, NULL);
    sigset_t blocked;
    sigset_t empty;
    sigemptyset(&blocked);
    sigemptyset(&empty);
    sigaddset(&blocked, SIGCHLD);
    sigprocmask(SIG_BLOCK, &blocked, NULL);
    fflush(stdout);
    pid_t child = fork();
    if (child == 0) {
        _exit(3);
    }
    sigsuspend(&empty);
    int status = 0;
    waitpid(child, &status, 0);
    sigprocmask(SIG_UNBLOCK, &blocked, NULL);
    signal(SIGCHLD, SIG_DFL);
    printf("MACOS-PROCESS sigchld same %d code %s status %d wait %d\n", child_pid == child,
           child_code == CLD_EXITED ? "exited" : "other", child_status, WEXITSTATUS(status));
}

static int wait_status(pid_t child) {
    int status = 0;
    waitpid(child, &status, 0);
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}

static void exec_family(void) {
    fflush(stdout);
    pid_t child = fork();
    if (child == 0) {
        char *const arguments[] = {"macos-process", "exec-child", "by execv", NULL};
        execv(self_path, arguments);
        _exit(100);
    }
    printf("MACOS-PROCESS execv status %d\n", wait_status(child));
    fflush(stdout);
    child = fork();
    if (child == 0) {
        setenv("PATH", "/nowhere:/bin", 1);
        execlp("macos-process", "macos-process", "exec-child", "by execlp", (char *)NULL);
        _exit(100);
    }
    printf("MACOS-PROCESS execlp status %d\n", wait_status(child));
    char *const missing[] = {"nowhere", NULL};
    int failed = execv("/bin/nowhere", missing);
    printf("MACOS-PROCESS exec missing %d %s\n", failed, errno == ENOENT ? "ENOENT" : "other");
}

static void spawned(void) {
    int ends[2];
    pipe(ends);
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, ends[1], 1);
    posix_spawn_file_actions_addinherit_np(&actions, 2);
    posix_spawnattr_t attributes;
    posix_spawnattr_init(&attributes);
    posix_spawnattr_setflags(&attributes, POSIX_SPAWN_CLOEXEC_DEFAULT);
    short flags = 0;
    posix_spawnattr_getflags(&attributes, &flags);
    char *const arguments[] = {"macos-process", "echo-child", NULL};
    pid_t child = 0;
    fflush(stdout);
    int started = posix_spawn(&child, self_path, &actions, &attributes, arguments, environ);
    posix_spawn_file_actions_destroy(&actions);
    posix_spawnattr_destroy(&attributes);
    close(ends[1]);
    char buffer[64];
    ssize_t length = 0;
    ssize_t got;
    while ((got = read(ends[0], buffer + length, sizeof buffer - 1 - length)) > 0) {
        length += got;
    }
    buffer[length] = 0;
    close(ends[0]);
    if (length > 0 && buffer[length - 1] == '\n') {
        buffer[length - 1] = 0;
    }
    printf("MACOS-PROCESS spawn %d flags %d read '%s' status %d\n", started,
           flags == POSIX_SPAWN_CLOEXEC_DEFAULT, buffer, wait_status(child));

    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, 0, "/usr/share/eggos/about", O_RDONLY, 0);
    char *const counting[] = {"macos-process", "count-child", NULL};
    fflush(stdout);
    started = posix_spawn(&child, self_path, &actions, NULL, counting, NULL);
    posix_spawn_file_actions_destroy(&actions);
    printf("MACOS-PROCESS spawn open %d status %d\n", started, wait_status(child));

    char *const nothing[] = {"nowhere", NULL};
    int missing = posix_spawnp(&child, "nowhere-to-be-found", NULL, NULL, nothing, environ);
    posix_spawnattr_init(&attributes);
    posix_spawnattr_setflags(&attributes, POSIX_SPAWN_START_SUSPENDED);
    int suspended = posix_spawn(&child, self_path, NULL, &attributes, arguments, environ);
    posix_spawnattr_destroy(&attributes);
    printf("MACOS-PROCESS spawnp missing %s suspended %s\n", missing == ENOENT ? "ENOENT" : "other",
           suspended == ENOTSUP ? "ENOTSUP" : "other");
}

static void shell(void) {
    fflush(stdout);
    FILE *output = popen("echo popen works", "r");
    char line[64] = "";
    if (output != NULL) {
        fgets(line, sizeof line, output);
    }
    size_t length = strlen(line);
    if (length > 0 && line[length - 1] == '\n') {
        line[length - 1] = 0;
    }
    int closed = output != NULL ? pclose(output) : -1;
    printf("MACOS-PROCESS popen '%s' status %d\n", line, WEXITSTATUS(closed));
    fflush(stdout);
    int status = system("exit 4");
    printf("MACOS-PROCESS system %d available %d\n", WEXITSTATUS(status), system(NULL) != 0);
}

static int count_input(void) {
    char buffer[256];
    ssize_t total = 0;
    ssize_t got;
    while ((got = read(0, buffer, sizeof buffer)) > 0) {
        total += got;
    }
    printf("MACOS-PROCESS count-child %s\n", total > 0 ? "read input" : "no input");
    return 0;
}

int main(int argc, char **argv) {
    if (argc >= 3 && strcmp(argv[1], "exec-child") == 0) {
        printf("MACOS-PROCESS exec-child %s\n", argv[2]);
        return 5;
    }
    if (argc >= 2 && strcmp(argv[1], "echo-child") == 0) {
        printf("spawned hello\n");
        return 0;
    }
    if (argc >= 2 && strcmp(argv[1], "count-child") == 0) {
        return count_input();
    }
    printf("MACOS-PROCESS main\n");
    handlers();
    alternate_stack();
    fork_and_pipe();
    killed_child();
    child_signal();
    exec_family();
    spawned();
    shell();
    printf("MACOS-PROCESS done\n");
    return 0;
}
