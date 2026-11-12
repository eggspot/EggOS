#include <cstdio>
#include <cstring>
#include <exception>
#include <new>
#include <typeinfo>

namespace {

int destroyed = 0;

struct Guard {
    const char *name;
    explicit Guard(const char *guard_name) : name(guard_name) {}
    ~Guard() {
        ++destroyed;
        std::printf("MACOS-CXX unwind %s\n", name);
    }
};

struct Failure : std::exception {
    int code;
    explicit Failure(int failure_code) : code(failure_code) {}
    const char *what() const noexcept override { return "failure"; }
};

struct Shape {
    virtual ~Shape() {}
    virtual const char *kind() const { return "shape"; }
};

struct Named {
    virtual ~Named() {}
    const char *label = "named";
};

struct Circle : Shape, Named {
    const char *kind() const override { return "circle"; }
};

struct Square : Shape {
    const char *kind() const override { return "square"; }
};

int counted() {
    static int calls = 0;
    return ++calls;
}

int first_value() {
    static const int value = counted() * 10;
    return value;
}

void throw_failure(int code) {
    Guard inner("inner");
    throw Failure(code);
}

void rethrow_inside() {
    try {
        throw_failure(7);
    } catch (Failure &) {
        std::printf("MACOS-CXX caught once\n");
        throw;
    }
}

void catches() {
    try {
        throw 42;
    } catch (int value) {
        std::printf("MACOS-CXX int %d\n", value);
    }
    try {
        Guard outer("outer");
        throw_failure(3);
    } catch (const std::exception &error) {
        std::printf("MACOS-CXX base %s destroyed=%d\n", error.what(), destroyed);
    }
    try {
        rethrow_inside();
    } catch (const Failure &failure) {
        std::printf("MACOS-CXX rethrown %d\n", failure.code);
    }
    try {
        throw "text";
    } catch (const char *text) {
        std::printf("MACOS-CXX pointer %s\n", text);
    }
    try {
        throw 2.5;
    } catch (...) {
        std::printf("MACOS-CXX any\n");
    }
    try {
        try {
            throw Failure(9);
        } catch (int) {
            std::printf("MACOS-CXX wrong\n");
        }
    } catch (Failure &failure) {
        std::printf("MACOS-CXX outer %d\n", failure.code);
    }
}

void casts() {
    Circle circle;
    Square square;
    Shape *shape = &circle;
    Circle *as_circle = dynamic_cast<Circle *>(shape);
    Square *as_square = dynamic_cast<Square *>(shape);
    Named *as_named = dynamic_cast<Named *>(shape);
    std::printf("MACOS-CXX cast %d %d %s %s\n", as_circle == &circle, as_square == nullptr,
                as_named ? as_named->label : "none", as_circle->kind());
    Shape *other = &square;
    std::printf("MACOS-CXX cast %d %s\n", dynamic_cast<Circle *>(other) == nullptr,
                other->kind());
    std::printf("MACOS-CXX typeid %d %d\n", typeid(*shape) == typeid(Circle),
                typeid(*other) == typeid(Circle));
}

void memory() {
    int *numbers = new int[4]{1, 2, 3, 4};
    int sum = numbers[0] + numbers[1] + numbers[2] + numbers[3];
    delete[] numbers;
    void *refused = operator new(static_cast<std::size_t>(-1) / 2, std::nothrow);
    std::printf("MACOS-CXX memory %d %d\n", sum, refused == nullptr);
    try {
        void *huge = operator new(static_cast<std::size_t>(-1) / 2);
        operator delete(huge);
        std::printf("MACOS-CXX wrong\n");
    } catch (const std::bad_alloc &error) {
        std::printf("MACOS-CXX bad_alloc %d\n", std::strlen(error.what()) > 0);
    }
}

}

int main() {
    int first = first_value();
    int second = first_value();
    int calls = counted();
    std::printf("MACOS-CXX statics %d %d %d\n", first, second, calls);
    catches();
    casts();
    memory();
    std::printf("MACOS-CXX done\n");
    return 0;
}
