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

enum Level: Int, CaseIterable {
    case easy = 0
    case hard = 1
    case expert = 2
}

final class Node {
    let name: String
    weak var parent: Node?
    init(name: String) { self.name = name }
}

protocol Named: AnyObject {
    var name: String { get }
}

final class Owner: Named {
    let name = "owner"
}

final class Holder {
    weak var named: Named?
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

@_semantics("optimize.sil.specialize.generic.never")
@inline(never)
func same<T: Hashable>(_ left: T, _ right: T) -> Bool {
    left == right && left.hashValue == right.hashValue
}

@_semantics("optimize.sil.specialize.generic.never")
@inline(never)
func rawValue<T: RawRepresentable>(of value: T) -> T.RawValue {
    value.rawValue
}

@inline(never)
func classify(_ value: Any) -> String {
    if let number = value as? Int { return "int \(number)" }
    if let text = value as? String { return "string \(text)" }
    if let node = value as? Node { return "node \(node.name)" }
    if let level = value as? Level { return "level \(level.rawValue)" }
    return "other"
}

@inline(never)
func deposit(_ amount: Int, into balance: inout Int, while work: () -> Void) {
    work()
    balance += amount
}

if CommandLine.argc > 2 {
    print("MACOS-SWIFT fatal")
    fatalError("egg cracked")
} else if CommandLine.argc > 1 {
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
    print("MACOS-SWIFT levels \(Level.allCases.count) \(same(Level.hard, Level.hard)) \(same(Level.easy, Level.expert)) \(same("egg", "egg")) \(same(7, 8)) raw \(rawValue(of: Level.expert))")
    print("MACOS-SWIFT casts \(classify(5)) \(classify("egg")) \(classify(Node(name: "nest"))) \(classify(Level.hard)) \(classify(2.5))")
    let parent = Node(name: "parent")
    let child = Node(name: "child")
    child.parent = parent
    let holder = Holder()
    withExtendedLifetime(Owner()) { owner in
        holder.named = owner
        print("MACOS-SWIFT weak \(child.parent?.name ?? "nil") \(holder.named?.name ?? "nil") \(parent.name)")
    }
    print("MACOS-SWIFT weak after \(holder.named?.name ?? "nil")")
    let text = "H\u{E9}llo, W\u{F6}rld"
    var letters: [Character: Int] = [:]
    for letter in text {
        letters[letter, default: 0] += 1
    }
    let index = text.index(text.startIndex, offsetBy: 4, limitedBy: text.endIndex)!
    let beyond = text.index(text.startIndex, offsetBy: 20, limitedBy: text.endIndex)
    let start = String(text[text.startIndex..<index])
    let lower = text.lowercased()
    print("MACOS-SWIFT characters \(text.count) \(letters.count) \(text[index]) \(start == "H\u{E9}ll") \(lower == "h\u{E9}llo, w\u{F6}rld") \(beyond == nil)")
    let roll = Int.random(in: 1...6)
    print("MACOS-SWIFT type \(type(of: child)) \(type(of: Level.easy)) random \((1...6).contains(roll))")
    print("MACOS-SWIFT done")
}
