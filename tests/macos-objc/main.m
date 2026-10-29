#import <Foundation/Foundation.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>

extern void *objc_retain(void *object);
extern void objc_release(void *object);

typedef struct {
    long first;
    long second;
    long third;
} Triple;

@interface Counter : NSObject
@property (nonatomic) NSInteger count;
@property (nonatomic, copy) NSString *name;
@property (nonatomic, strong) NSMutableArray *items;
+ (instancetype)counterNamed:(NSString *)name;
- (instancetype)initWithName:(NSString *)name;
- (void)increment;
- (NSInteger)value;
- (Triple)triple;
@end

@interface Counter (Missing)
- (void)frobnicate;
@end

@implementation Counter

+ (void)load {
    printf("MACOS-OBJC load Counter\n");
}

+ (void)initialize {
    if (self == [Counter class]) {
        printf("MACOS-OBJC initialize Counter\n");
    }
}

+ (instancetype)counterNamed:(NSString *)name {
    Counter *counter = [super new];
    counter.name = name;
    return counter;
}

- (instancetype)initWithName:(NSString *)name {
    self = [super init];
    if (self) {
        _name = [name copy];
        _items = [NSMutableArray array];
    }
    return self;
}

- (void)increment {
    self.count += 1;
    [self.items addObject:@(self.count)];
}

- (NSInteger)value {
    return _count;
}

- (Triple)triple {
    Triple triple = {self.count, self.count * 2, self.count * 3};
    return triple;
}

- (NSString *)description {
    return [NSString stringWithFormat:@"<Counter %@ %ld>", self.name, (long)self.count];
}

- (void)dealloc {
    printf("MACOS-OBJC dealloc %s\n", self.name.UTF8String);
}

@end

@interface NSString (EggShout)
- (NSString *)egg_shouted;
@end

@implementation NSString (EggShout)

+ (void)load {
    printf("MACOS-OBJC load NSString(EggShout)\n");
}

- (NSString *)egg_shouted {
    return [[self uppercaseString] stringByAppendingString:@"!"];
}

@end

@interface Noisy : NSObject
+ (instancetype)noisy;
@end

@implementation Noisy

+ (instancetype)noisy {
    return [[self alloc] init];
}

- (void)dealloc {
    printf("MACOS-OBJC dealloc Noisy on its thread\n");
}

@end

static void *worker(void *argument) {
    Noisy *noisy = [Noisy noisy];
    printf("MACOS-OBJC thread %s\n", [noisy class] == [Noisy class] ? "noisy" : "other");
    return argument;
}

static void say(NSString *line) {
    printf("MACOS-OBJC %s\n", line.UTF8String);
}

static int bench(void) {
    Counter *counter = [[Counter alloc] initWithName:@"bench"];
    const long rounds = 1000000;
    mach_timebase_info_data_t timebase;
    mach_timebase_info(&timebase);
    volatile long sum = 0;
    uint64_t start = mach_absolute_time();
    for (long round = 0; round < rounds; round++) {
        sum += [counter value];
    }
    uint64_t sends = mach_absolute_time() - start;
    void *raw = (__bridge void *)counter;
    start = mach_absolute_time();
    for (long round = 0; round < rounds; round++) {
        objc_release(objc_retain(raw));
    }
    uint64_t pairs = mach_absolute_time() - start;
    double scale = (double)timebase.numer / timebase.denom / rounds;
    printf("MACOS-OBJC bench send %.1f ns, retain and release %.1f ns (%ld)\n", sends * scale,
           pairs * scale, (long)sum);
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "unrecognized") == 0) {
        Counter *counter = [Counter new];
        [counter frobnicate];
        return 1;
    }
    if (argc > 1 && strcmp(argv[1], "unimplemented") == 0) {
        say([[NSDate date] description]);
        return 1;
    }
    if (argc > 1 && strcmp(argv[1], "bench") == 0) {
        return bench();
    }
    @autoreleasepool {
        say(@"main");
        fflush(stdout);
        NSLog(@"MACOS-OBJC NSLog %@ %d %s %@", @"hello", 42, "world", nil);
        @autoreleasepool {
            Counter *counter = [[Counter alloc] initWithName:@"egg"];
            [counter increment];
            [counter increment];
            say(counter.description);
            say([NSString stringWithFormat:@"items %@", [counter.items componentsJoinedByString:@","]]);
            Triple triple = [counter triple];
            printf("MACOS-OBJC triple %ld %ld %ld\n", triple.first, triple.second, triple.third);
            say([NSString stringWithFormat:@"kinds %d %d %d %d", [counter isKindOfClass:[NSObject class]],
                                           [counter isMemberOfClass:[NSObject class]],
                                           [counter respondsToSelector:@selector(increment)],
                                           [counter respondsToSelector:@selector(frobnicate)]]);
            say([NSString stringWithFormat:@"class %@ %@ %@", NSStringFromClass([counter class]),
                                           NSStringFromSelector(@selector(initWithName:)),
                                           [Counter description]]);
        }
        say(@"counter pool done");
        @autoreleasepool {
            Counter *named = [Counter counterNamed:@"spam"];
            say([NSString stringWithFormat:@"named %@ %ld", named.name, (long)named.count]);
        }
        say(@"named pool done");
        say([NSString stringWithFormat:@"shout %@", [@"abc" egg_shouted]]);
        NSString *wide = @"héllo";
        printf("MACOS-OBJC wide %lu %s %d\n", (unsigned long)wide.length, wide.UTF8String,
               [wide characterAtIndex:1]);
        NSString *built = [NSString stringWithUTF8String:"h\xc3\xa9llo w\xc3\xb6rld"];
        printf("MACOS-OBJC built %lu %s %s %s\n", (unsigned long)built.length,
               [built substringFromIndex:6].UTF8String,
               [built isEqualToString:@"héllo wörld"] ? "equal" : "different",
               [built hasPrefix:@"hé"] ? "prefix" : "no-prefix");
        say([[@"a,b,,c" componentsSeparatedByString:@","] componentsJoinedByString:@"|"]);
        NSArray *array = @[ @1, @"two", @3.5, @[ @4, @"five" ], @{@"k" : @"v"} ];
        say([NSString stringWithFormat:@"array %@", array]);
        NSDictionary *dictionary = @{@"zeta" : @1, @"alpha" : @YES, @"mid" : @"a b", @"Beta" : @2.5f};
        say([NSString stringWithFormat:@"dictionary %@", dictionary]);
        say([NSString stringWithFormat:@"empty %@ %@", @[], @{}]);
        for (id item in @[ @1, @"two", @3.5 ]) {
            say([NSString stringWithFormat:@"item %@", item]);
        }
        NSMutableArray *mutable = [NSMutableArray array];
        [mutable addObject:@"x"];
        mutable[1] = @"y";
        [mutable insertObject:@"w" atIndex:0];
        [mutable removeObjectAtIndex:1];
        say([NSString stringWithFormat:@"mutable %@ %lu", [mutable componentsJoinedByString:@"+"],
                                       (unsigned long)mutable.count]);
        NSMutableDictionary *table = [NSMutableDictionary dictionary];
        table[@"b"] = @2;
        table[@"a"] = @1;
        table[@"b"] = @3;
        [table removeObjectForKey:@"zz"];
        say([NSString stringWithFormat:@"table %lu %@ %@", (unsigned long)table.count, table[@"b"],
                                       [[table.allKeys sortedArrayUsingSelector:@selector(compare:)]
                                           componentsJoinedByString:@","]]);
        say([NSString stringWithFormat:@"table %@", table]);
        NSMutableString *text = [NSMutableString stringWithString:@"egg"];
        [text appendString:@"s"];
        [text appendFormat:@" %d", 12];
        [text insertString:@"big " atIndex:0];
        say([NSString stringWithFormat:@"text %@ %lu", text, (unsigned long)text.length]);
        say([NSString stringWithFormat:@"numbers %@ %@ %@ %@ %@ %@ %@", @42, @(-7), @0.1, @(3.25f), @YES,
                                       @(1ULL << 63), @'c']);
        say([NSString stringWithFormat:@"compare %d %d %ld %d", [@1 isEqual:@1.0], [@"a" isEqual:@"a"],
                                       (long)[@"apple" compare:@"banana"], @"42".intValue]);
        NSBundle *bundle = [NSBundle mainBundle];
        NSString *directory = [bundle.bundlePath stringByAppendingString:@"/"];
        say([NSString stringWithFormat:@"bundle %d %@ %lu", [bundle.executablePath hasPrefix:directory],
                                       bundle.bundleIdentifier, (unsigned long)bundle.infoDictionary.count]);
        pthread_t thread;
        pthread_create(&thread, NULL, worker, NULL);
        pthread_join(thread, NULL);
        say(@"joined");
    }
    say(@"done");
    return 0;
}
