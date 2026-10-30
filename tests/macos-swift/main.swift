protocol Shape {
    var name: String { get }
    func area() -> Double
}

struct Square: Shape {
    let side: Double
    var name: String { "square" }
    func area() -> Double { side * side }
}

struct Circle: Shape {
    let radius: Double
    var name: String { "circle" }
    func area() -> Double { 3.0 * radius * radius }
}

final class Egg {
    let label: String
    init(label: String) {
        self.label = label
        print("MACOS-SWIFT hatch \(label)")
    }
    deinit {
        print("MACOS-SWIFT deinit \(label)")
    }
}

final class Counter {
    private(set) var count = 0
    func bump() { count += 1 }
}

final class Account {
    var balance = 0
}

func largest<T: Shape>(_ shapes: [T]) -> T? {
    var best: T?
    for shape in shapes where best == nil || shape.area() > best!.area() {
        best = shape
    }
    return best
}

@inline(never)
func hatch(_ label: String) -> Egg {
    Egg(label: label)
}

@inline(never)
func describe(_ shapes: [any Shape]) -> String {
    var text = ""
    for shape in shapes {
        if !text.isEmpty { text += "," }
        text += shape.name
    }
    return text
}

@inline(never)
func deposit(_ amount: Int, into balance: inout Int, while work: () -> Void) {
    work()
    balance += amount
}

if CommandLine.argc > 1 {
    let account = Account()
    print("MACOS-SWIFT conflict")
    deposit(1, into: &account.balance) { account.balance += 1 }
    print("MACOS-SWIFT not reached \(account.balance)")
} else {
    print("MACOS-SWIFT main")
    let counter = Counter()
    var names: [String] = []
    var areas: [String: Double] = [:]
    let squares = [Square(side: 2), Square(side: 3)]
    for square in squares {
        counter.bump()
        names.append(square.name)
        areas[square.name, default: 0] += square.area()
    }
    let circles = [Circle(radius: 1)]
    if let big = largest(circles) { areas[big.name] = big.area() }
    print("MACOS-SWIFT count \(counter.count) names \(names.count) square \(areas["square"] ?? 0) circle \(areas["circle"] ?? 0) largest \(largest(squares)?.side ?? 0)")
    print("MACOS-SWIFT shapes \(describe([Square(side: 1), Circle(radius: 2), Square(side: 4)]))")
    var long = "MACOS-SWIFT grow"
    for word in ["eggs", "are", "longer", "than", "fifteen", "bytes"] {
        long += " " + word
    }
    print(long)
    var keyed: [String: Int] = [:]
    for (index, word) in ["a key longer than fifteen bytes", "short", "another long key for hashing"].enumerated() {
        keyed[word] = index
    }
    print("MACOS-SWIFT keyed \(keyed.count) \(keyed["short"] ?? -1) \(keyed["another long key for hashing"] ?? -1) \(keyed["missing"] ?? -1)")
    print("MACOS-SWIFT compare \(long < "MACOS-SWIFT z") \("abc" == "ab" + "c") \(long == "short")")
    do {
        let egg = hatch("first")
        print("MACOS-SWIFT holding \(egg.label)")
    }
    print("MACOS-SWIFT ints \(-42) \(Int.max) \(UInt8(200)) doubles \(0.1) \(1.5e300) \(-2.0) \(1.0 / 3.0)")
    print("MACOS-SWIFT", "items", 3, separator: "|")
    print("MACOS-SWIFT done")
}
