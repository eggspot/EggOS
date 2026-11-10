#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <chrono>
#include <condition_variable>
#include <exception>
#include <functional>
#include <mutex>
#include <stdexcept>
#include <string>
#include <system_error>
#include <thread>
#include <typeinfo>
#include <vector>

struct Shape {
    virtual ~Shape() {}
};

struct Circle : Shape {};

struct Square : Shape {};

struct Counted {
    explicit Counted(int number) : value(number) {}
    Counted(const Counted &other) : value(other.value) { copies++; }
    ~Counted() {}
    int value;
    static int copies;
};

int Counted::copies;

struct Witness {
    ~Witness() { seen = std::uncaught_exceptions(); }
    static int seen;
};

int Witness::seen = -1;

static Shape *shape_of(int kind)
{
    static Circle circle;
    static Square square;
    if (kind == 0) {
        return &circle;
    }
    return &square;
}

static void standard_exceptions_from_the_runtime()
{
    try {
        Shape &shape = *shape_of(1);
        Circle &circle = dynamic_cast<Circle &>(shape);
        printf("EGGCPP2 cast %p\n", (void *)&circle);
    } catch (const std::bad_cast &error) {
        printf("EGGCPP2 bad_cast %s, %s\n", error.what(), typeid(error).name());
    }
    try {
        Shape *nothing = shape_of(0) == shape_of(1) ? shape_of(0) : nullptr;
        printf("EGGCPP2 typeid %s\n", typeid(*nothing).name());
    } catch (const std::exception &error) {
        printf("EGGCPP2 bad_typeid %s, %s\n", error.what(), typeid(error).name());
    }
    std::vector<int> numbers(3, 7);
    try {
        printf("EGGCPP2 at %d\n", numbers.at(10));
    } catch (const std::logic_error &error) {
        printf("EGGCPP2 out_of_range %s, %s\n", error.what(), typeid(error).name());
    }
    std::string text("egg");
    try {
        text.reserve(text.max_size() + 1);
        printf("EGGCPP2 reserved %zu\n", text.capacity());
    } catch (const std::length_error &error) {
        printf("EGGCPP2 length_error %s\n", error.what());
    }
    std::function<int(int)> empty;
    try {
        printf("EGGCPP2 called %d\n", empty(1));
    } catch (const std::exception &error) {
        printf("EGGCPP2 bad_function_call %s\n", error.what());
    }
}

static void exception_pointers()
{
    std::exception_ptr kept;
    try {
        throw Counted(5);
    } catch (...) {
        kept = std::current_exception();
    }
    int copies_before = Counted::copies;
    for (int round = 0; round < 2; round++) {
        try {
            std::rethrow_exception(kept);
        } catch (Counted &caught) {
            caught.value++;
            printf("EGGCPP2 rethrown %d\n", caught.value);
        }
    }
    printf("EGGCPP2 exception_ptr copies %d, kept %d\n", Counted::copies - copies_before,
           kept ? 1 : 0);
    std::exception_ptr made = std::make_exception_ptr(std::runtime_error("made"));
    try {
        std::rethrow_exception(made);
    } catch (const std::runtime_error &error) {
        printf("EGGCPP2 made %s, equal %d\n", error.what(), made == kept ? 1 : 0);
    }
}

static void uncaught_count()
{
    try {
        Witness witness;
        throw 1;
    } catch (int) {
        printf("EGGCPP2 uncaught during unwinding %d, in catch %d\n", Witness::seen,
               std::uncaught_exceptions());
    }
}

static void threads()
{
    std::mutex lock;
    long total = 0;
    std::vector<std::thread> workers;
    for (int index = 0; index < 4; index++) {
        workers.emplace_back([&lock, &total] {
            for (int step = 0; step < 1000; step++) {
                std::lock_guard<std::mutex> guard(lock);
                total++;
            }
        });
    }
    for (std::thread &worker : workers) {
        worker.join();
    }
    printf("EGGCPP2 threads total %ld\n", total);
    std::condition_variable ready;
    bool done = false;
    int handed = 0;
    std::thread producer([&] {
        std::lock_guard<std::mutex> guard(lock);
        handed = 42;
        done = true;
        ready.notify_one();
    });
    {
        std::unique_lock<std::mutex> guard(lock);
        ready.wait(guard, [&] { return done; });
    }
    producer.join();
    printf("EGGCPP2 condition handed %d\n", handed);
    std::recursive_mutex nested;
    nested.lock();
    bool again = nested.try_lock();
    nested.unlock();
    if (again) {
        nested.unlock();
    }
    printf("EGGCPP2 recursive %d, cpus %d\n", again ? 1 : 0,
           std::thread::hardware_concurrency() > 0 ? 1 : 0);
    auto before = std::chrono::steady_clock::now();
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
    auto slept = std::chrono::duration_cast<std::chrono::milliseconds>(
        std::chrono::steady_clock::now() - before);
    printf("EGGCPP2 slept at least 20 ms %d\n", slept.count() >= 20 ? 1 : 0);
    std::thread joined([] {});
    joined.join();
    try {
        joined.join();
    } catch (const std::system_error &error) {
        printf("EGGCPP2 system_error %d %s, %s, %s\n", error.code().value(),
               error.code().category().name(),
               error.code() == std::errc::invalid_argument ? "invalid argument" : "other",
               error.code().message().c_str());
    }
}

static int thrower(int value)
{
    if (value >= 0) {
        throw value;
    }
    return value;
}

static void bench(int rounds)
{
    LARGE_INTEGER start;
    LARGE_INTEGER end;
    LARGE_INTEGER frequency;
    int caught = 0;
    if (rounds < 1) {
        rounds = 1;
    }
    QueryPerformanceFrequency(&frequency);
    QueryPerformanceCounter(&start);
    for (int round = 0; round < rounds; round++) {
        try {
            thrower(round);
        } catch (int value) {
            caught += value >= 0;
        }
    }
    QueryPerformanceCounter(&end);
    double scale = 1e9 / (double)frequency.QuadPart / rounds;
    printf("EGGCPP2 bench cxx_throw_catch %.1f ns (%d)\n",
           (double)(end.QuadPart - start.QuadPart) * scale, caught);
}

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "bench") == 0) {
        bench(argc > 2 ? atoi(argv[2]) : 1000);
        return 0;
    }
    standard_exceptions_from_the_runtime();
    exception_pointers();
    uncaught_count();
    threads();
    printf("EGGCPP2 done\n");
    return 0;
}
