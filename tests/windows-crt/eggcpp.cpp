#include <windows.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <stdexcept>

static int destroyed;
static int vectored_calls;
static int finally_abnormal;
static int interrupted;
static ULONG_PTR access_kind;
static volatile int zero;

struct Tracker {
    explicit Tracker(int value) : id(value) {}
    ~Tracker() { destroyed++; }
    int id;
};

struct Base {
    virtual ~Base() {}
    virtual const char *name() const { return "base"; }
};

struct Derived : Base {
    explicit Derived(int number) : value(number) {}
    const char *name() const override { return "derived"; }
    int value;
};

struct Counted {
    explicit Counted(int number) : value(number) {}
    Counted(const Counted &other) : value(other.value) { copies++; }
    static int copies;
    int value;
};

int Counted::copies = 0;

__declspec(noinline) static void throw_int(int value)
{
    throw value;
}

__declspec(noinline) static void throw_derived(int value)
{
    Tracker inner(value);
    throw Derived(value);
}

__declspec(noinline) static void nested_frames(int depth)
{
    Tracker local(depth);
    if (depth == 0) {
        throw std::runtime_error("deep");
    }
    nested_frames(depth - 1);
}

static void check_int(void)
{
    try {
        throw_int(7);
    } catch (int value) {
        printf("EGGCPP caught int %d\n", value);
    }
}

static void check_hierarchy(void)
{
    destroyed = 0;
    try {
        throw_derived(5);
    } catch (const Base &base) {
        printf("EGGCPP caught %s through base, destroyed %d\n", base.name(), destroyed);
    }
}

static void check_unwinding(void)
{
    destroyed = 0;
    try {
        nested_frames(3);
    } catch (const std::exception &error) {
        printf("EGGCPP caught %s after %d destructors\n", error.what(), destroyed);
    }
}

static void check_rethrow(void)
{
    try {
        try {
            throw_int(42);
        } catch (...) {
            printf("EGGCPP rethrowing\n");
            throw;
        }
    } catch (int value) {
        printf("EGGCPP rethrown %d\n", value);
    }
}

static void check_catch_by_value(void)
{
    Counted::copies = 0;
    try {
        throw Counted(9);
    } catch (Counted counted) {
        printf("EGGCPP caught by value %d %s\n", counted.value,
               Counted::copies > 0 ? "copied" : "not copied");
    }
}

static void check_throw_in_catch(void)
{
    destroyed = 0;
    try {
        Tracker outer(1);
        try {
            throw_int(1);
        } catch (int) {
            Tracker in_catch(2);
            throw 2.5;
        }
    } catch (double value) {
        printf("EGGCPP caught %.1f from a catch block, destroyed %d\n", value, destroyed);
    }
}

static int access_filter(EXCEPTION_POINTERS *pointers)
{
    if (pointers->ExceptionRecord->ExceptionCode != EXCEPTION_ACCESS_VIOLATION) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    access_kind = pointers->ExceptionRecord->ExceptionInformation[0];
    return EXCEPTION_EXECUTE_HANDLER;
}

static void check_access_violation(void)
{
    volatile int *nothing = NULL;
    __try {
        *nothing = 1;
        printf("EGGCPP wrote through a null pointer\n");
    } __except (access_filter(GetExceptionInformation())) {
        printf("EGGCPP access violation %08lx write %d\n", GetExceptionCode(), (int)access_kind);
    }
}

__declspec(noinline) static int divide(int numerator, int denominator)
{
    return numerator / denominator;
}

static void check_divide_by_zero(void)
{
    __try {
        printf("EGGCPP quotient %d\n", divide(10, zero));
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        printf("EGGCPP divide by zero %08lx\n", GetExceptionCode());
    }
}

__declspec(noinline) static void raise_inside_finally(void)
{
    __try {
        RaiseException(0xE0000001, 0, 0, NULL);
    } __finally {
        finally_abnormal = AbnormalTermination();
    }
}

static void check_finally(void)
{
    __try {
        raise_inside_finally();
    } __except (GetExceptionCode() == 0xE0000001 ? EXCEPTION_EXECUTE_HANDLER
                                                  : EXCEPTION_CONTINUE_SEARCH) {
        printf("EGGCPP finally ran %d before except\n", finally_abnormal);
    }
}

static LONG CALLBACK on_vectored(EXCEPTION_POINTERS *pointers)
{
    if (pointers->ExceptionRecord->ExceptionCode != 0xE0000002) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    vectored_calls++;
    return EXCEPTION_CONTINUE_EXECUTION;
}

static void check_vectored(void)
{
    PVOID handle = AddVectoredExceptionHandler(1, on_vectored);
    RaiseException(0xE0000002, 0, 0, NULL);
    ULONG removed = RemoveVectoredExceptionHandler(handle);
    printf("EGGCPP vectored continued %d removed %lu\n", vectored_calls, removed);
}

static void check_continue_execution(void)
{
    volatile int resumed = 0;
    __try {
        RaiseException(0xE0000003, 0, 0, NULL);
        resumed = 1;
    } __except (EXCEPTION_CONTINUE_EXECUTION) {
        resumed = 2;
    }
    printf("EGGCPP filter continued %d\n", resumed);
}

static void check_illegal_instruction(void)
{
#if defined(_M_ARM64)
    static const unsigned char instruction[] = {0x00, 0x00, 0x00, 0x00};
#else
    static const unsigned char instruction[] = {0x0f, 0x0b};
#endif
    void *code = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
    if (code == NULL) {
        printf("EGGCPP no executable memory\n");
        return;
    }
    memcpy(code, instruction, sizeof instruction);
    FlushInstructionCache(GetCurrentProcess(), code, sizeof instruction);
    __try {
        ((void (*)(void))code)();
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        printf("EGGCPP illegal instruction %08lx\n", GetExceptionCode());
    }
    VirtualFree(code, 0, MEM_RELEASE);
}

static void on_interrupt(int number)
{
    interrupted = number;
}

static void check_signal(void)
{
    signal(SIGINT, on_interrupt);
    raise(SIGINT);
    printf("EGGCPP signal %d\n", interrupted);
}

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "uncaught") == 0) {
        printf("EGGCPP throwing uncaught\n");
        fflush(stdout);
        throw_int(3);
    }
    check_int();
    check_hierarchy();
    check_unwinding();
    check_rethrow();
    check_catch_by_value();
    check_throw_in_catch();
    check_access_violation();
    check_divide_by_zero();
    check_finally();
    check_vectored();
    check_continue_execution();
    check_illegal_instruction();
    check_signal();
    printf("EGGCPP done\n");
    return 0;
}
