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

@interface Weakling : NSObject
@end

@implementation Weakling

- (void)dealloc {
    printf("MACOS-OBJC dealloc Weakling\n");
}

@end

@interface Thrower : NSObject
- (void)failWith:(int)code;
- (void)passThrough:(int)code;
@end

@implementation Thrower

- (void)failWith:(int)code {
    [NSException raise:@"EggRaise" format:@"code %d", code];
}

- (void)passThrough:(int)code {
    @try {
        [self failWith:code];
    } @finally {
        printf("MACOS-OBJC passing %d\n", code);
    }
}

@end

@interface Ticker : NSObject
- (void)tick:(NSTimer *)timer;
@end

@implementation Ticker

- (void)tick:(NSTimer *)timer {
    printf("MACOS-OBJC tick %s\n", [timer.userInfo UTF8String]);
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

static void blocks(void) {
    int base = 40;
    int (^add)(int) = ^(int value) {
        return base + value;
    };
    printf("MACOS-OBJC block %d\n", add(2));
    __block int counter = 0;
    void (^bump)(void) = ^{
        counter += 1;
    };
    bump();
    bump();
    void (^copied)(void) = [bump copy];
    copied();
    printf("MACOS-OBJC byref %d\n", counter);
    NSMutableArray *calls = [NSMutableArray array];
    [@[ @"a", @"b", @"c" ] enumerateObjectsUsingBlock:^(id item, NSUInteger index, BOOL *stop) {
        [calls addObject:[NSString stringWithFormat:@"%@%lu", item, (unsigned long)index]];
        if (index == 1) {
            *stop = YES;
        }
    }];
    say([NSString stringWithFormat:@"enumerate %@", [calls componentsJoinedByString:@","]]);
    NSString *captured = [NSString stringWithFormat:@"cap%d", 7];
    NSString * (^later)(void) = ^{
        return captured;
    };
    NSString * (^kept)(void) = [later copy];
    say([NSString stringWithFormat:@"captured %@", kept()]);
}

static void weak_references(void) {
    __weak Weakling *weak = nil;
    @autoreleasepool {
        __attribute__((objc_precise_lifetime)) Weakling *strong = [Weakling new];
        weak = strong;
        printf("MACOS-OBJC weak %s\n", weak ? "set" : "nil");
    }
    printf("MACOS-OBJC weak %s\n", weak ? "set" : "nil");
}

static void exceptions(void) {
    @try {
        @throw [NSException exceptionWithName:@"EggError" reason:@"cracked" userInfo:nil];
    } @catch (NSException *exception) {
        say([NSString stringWithFormat:@"caught %@ %@", exception.name, exception.reason]);
    } @finally {
        say(@"finally");
    }
    Thrower *thrower = [Thrower new];
    @try {
        [thrower passThrough:3];
    } @catch (NSException *exception) {
        say([NSString stringWithFormat:@"caught %@ %@", exception.name, exception.reason]);
    }
    @try {
        @try {
            [thrower failWith:4];
        } @catch (NSException *exception) {
            say([NSString stringWithFormat:@"inner %@", exception.name]);
            @throw;
        }
    } @catch (id any) {
        say([NSString stringWithFormat:@"outer %@", [any reason]]);
    }
    @try {
        @throw @"plain string";
    } @catch (NSException *exception) {
        say(@"wrong catch");
    } @catch (id any) {
        say([NSString stringWithFormat:@"caught any %@", any]);
    }
}

static void foundation_classes(void) {
    NSSet *set = [NSSet setWithObjects:@"b", @"a", @"b", nil];
    printf("MACOS-OBJC set %lu %d %d\n", (unsigned long)set.count, [set containsObject:@"a"],
           [set containsObject:@"z"]);
    NSMutableSet *mutable = [set mutableCopy];
    [mutable removeObject:@"b"];
    [mutable addObject:@"c"];
    [mutable addObject:@"a"];
    NSArray *sorted = [mutable.allObjects sortedArrayUsingSelector:@selector(compare:)];
    say([NSString stringWithFormat:@"set %@", [sorted componentsJoinedByString:@","]]);
    NSDate *epoch = [NSDate dateWithTimeIntervalSince1970:0];
    NSDate *later = [epoch dateByAddingTimeInterval:86400 * 365];
    say([NSString stringWithFormat:@"date %@ %@ %ld %.0f", epoch, later, (long)[epoch compare:later],
                                   [later timeIntervalSinceDate:epoch]]);
    NSData *data = [NSData dataWithBytes:"abc" length:3];
    NSMutableData *grown = [data mutableCopy];
    [grown appendBytes:"de" length:2];
    say([NSString stringWithFormat:@"data %@ %lu %d", data, (unsigned long)grown.length,
                                   [data isEqual:[NSData dataWithBytes:"abc" length:3]]]);
    NSError *error = [NSError errorWithDomain:NSPOSIXErrorDomain
                                         code:2
                                     userInfo:@{NSLocalizedDescriptionKey : @"Egg missing"}];
    say([NSString stringWithFormat:@"error %@ %ld %@", error.domain, (long)error.code,
                                   error.localizedDescription]);
    NSProcessInfo *process = [NSProcessInfo processInfo];
    say([NSString stringWithFormat:@"process %@ %@ %d %d", process.processName,
                                   process.arguments.lastObject, process.processIdentifier > 0,
                                   process.activeProcessorCount >= 1]);
    NSString *text = @"hello egg world";
    NSRange found = [text rangeOfString:@"egg"];
    NSRange missing = [text rangeOfString:@"spam"];
    NSRange folded = [text rangeOfString:@"EGG" options:NSCaseInsensitiveSearch];
    NSRange last = [text rangeOfString:@"o" options:NSBackwardsSearch];
    printf("MACOS-OBJC range %lu %lu %d %lu %lu\n", (unsigned long)found.location,
           (unsigned long)found.length, missing.location == NSNotFound,
           (unsigned long)folded.location, (unsigned long)last.location);
}

static void timers(void) {
    __block int ticks = 0;
    NSTimer *timer = [NSTimer scheduledTimerWithTimeInterval:0.01
                                                     repeats:YES
                                                       block:^(NSTimer *fired) {
                                                         ticks += 1;
                                                         if (ticks == 3) {
                                                             [fired invalidate];
                                                         }
                                                       }];
    [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.5]];
    printf("MACOS-OBJC timer %d %d\n", ticks, timer.isValid);
    Ticker *ticker = [Ticker new];
    [NSTimer scheduledTimerWithTimeInterval:0.02
                                     target:ticker
                                   selector:@selector(tick:)
                                   userInfo:@"once"
                                    repeats:NO];
    [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.2]];
}

static int language(void) {
    @autoreleasepool {
        blocks();
        weak_references();
        exceptions();
        foundation_classes();
        timers();
        say(@"language done");
    }
    return 0;
}

static void uncaught(NSException *exception) {
    printf("MACOS-OBJC handler %s\n", exception.name.UTF8String);
    fflush(stdout);
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
        say([[NSUUID UUID] UUIDString]);
        return 1;
    }
    if (argc > 1 && strcmp(argv[1], "language") == 0) {
        return language();
    }
    if (argc > 1 && strcmp(argv[1], "uncaught") == 0) {
        NSSetUncaughtExceptionHandler(&uncaught);
        [NSException raise:@"EggFatal" format:@"no catch for %d", 9];
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
