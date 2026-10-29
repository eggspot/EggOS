#import <AppKit/AppKit.h>

static const NSRect CONTENT = {{0, 0}, {400, 300}};
static const NSRect PAINT = {{0, 48}, {400, 252}};
static const NSRect FIELD = {{8, 10}, {184, 28}};
static const NSRect CLEAR = {{200, 10}, {96, 28}};
static const NSRect INFO = {{304, 8}, {88, 32}};
static const NSRect OVAL = {{364, 216}, {24, 24}};
static const NSPoint CAPTION = {8, 228};
static const NSPoint RULE_FROM = {8, 224};
static const NSPoint RULE_TO = {192, 224};
static const CGFloat SQUARE = 8;

enum { MAX_SQUARES = 8 };

@interface PaintView : NSView {
    NSPoint _squares[MAX_SQUARES];
    NSInteger _count;
}
- (void)clear;
@end

@implementation PaintView

- (BOOL)acceptsFirstResponder {
    return YES;
}

- (void)drawRect:(NSRect)dirtyRect {
    NSRect bounds = self.bounds;
    [[NSColor textBackgroundColor] setFill];
    NSRectFill(bounds);
    NSDictionary *attributes = @{NSForegroundColorAttributeName : [NSColor secondaryLabelColor]};
    [@"Click to paint, c clears" drawAtPoint:CAPTION withAttributes:attributes];
    [[NSColor separatorColor] setStroke];
    NSBezierPath *rule = [NSBezierPath bezierPath];
    [rule moveToPoint:RULE_FROM];
    [rule lineToPoint:RULE_TO];
    [rule stroke];
    NSBezierPath *egg = [NSBezierPath bezierPathWithOvalInRect:OVAL];
    [[NSColor systemYellowColor] setFill];
    [egg fill];
    [[NSColor labelColor] setStroke];
    [egg stroke];
    [[NSColor systemBlueColor] setFill];
    for (NSInteger index = 0; index < _count; index++) {
        NSPoint centre = _squares[index];
        NSRectFill(NSMakeRect(centre.x - SQUARE / 2, centre.y - SQUARE / 2, SQUARE, SQUARE));
    }
}

- (void)mouseDown:(NSEvent *)event {
    if (_count == MAX_SQUARES) {
        return;
    }
    _squares[_count] = [self convertPoint:event.locationInWindow fromView:nil];
    _count += 1;
    [self setNeedsDisplay:YES];
}

- (void)keyDown:(NSEvent *)event {
    if ([event.charactersIgnoringModifiers isEqualToString:@"c"]) {
        [self clear];
    } else {
        [super keyDown:event];
    }
}

- (void)clear {
    _count = 0;
    [self setNeedsDisplay:YES];
}

@end

@interface AppDelegate : NSObject <NSApplicationDelegate>
@property(nonatomic, strong) NSWindow *window;
@property(nonatomic, strong) PaintView *paint;
@property(nonatomic, strong) NSTextField *field;
@end

@implementation AppDelegate

- (NSMenu *)mainMenu {
    NSMenu *bar = [[NSMenu alloc] initWithTitle:@""];
    NSMenu *app = [[NSMenu alloc] initWithTitle:@"mac-paint"];
    [app addItemWithTitle:@"About mac-paint" action:@selector(info:) keyEquivalent:@""];
    [app addItem:[NSMenuItem separatorItem]];
    [app addItemWithTitle:@"Quit mac-paint" action:@selector(terminate:) keyEquivalent:@"q"];
    NSMenu *file = [[NSMenu alloc] initWithTitle:@"File"];
    [file addItemWithTitle:@"Clear" action:@selector(clear:) keyEquivalent:@"k"];
    [file addItemWithTitle:@"Close" action:@selector(performClose:) keyEquivalent:@"w"];
    NSMenu *help = [[NSMenu alloc] initWithTitle:@"Help"];
    [help addItemWithTitle:@"mac-paint Help" action:nil keyEquivalent:@""];
    for (NSMenu *menu in @[ app, file, help ]) {
        NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:menu.title action:nil keyEquivalent:@""];
        item.submenu = menu;
        [bar addItem:item];
    }
    return bar;
}

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    NSWindowStyleMask style = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                              NSWindowStyleMaskMiniaturizable;
    self.window = [[NSWindow alloc] initWithContentRect:CONTENT
                                              styleMask:style
                                                backing:NSBackingStoreBuffered
                                                  defer:NO];
    self.window.title = @"mac-paint";
    NSView *content = self.window.contentView;
    self.paint = [[PaintView alloc] initWithFrame:PAINT];
    [content addSubview:self.paint];
    self.field = [NSTextField textFieldWithString:@""];
    self.field.frame = FIELD;
    self.field.placeholderString = @"Title";
    self.field.target = self;
    self.field.action = @selector(rename:);
    [content addSubview:self.field];
    NSButton *clear = [NSButton buttonWithTitle:@"Clear" target:self action:@selector(clear:)];
    clear.frame = CLEAR;
    [content addSubview:clear];
    NSButton *info = [NSButton buttonWithTitle:@"Info" target:self action:@selector(info:)];
    info.frame = INFO;
    info.keyEquivalent = @"\r";
    [content addSubview:info];
    [NSApp setMainMenu:[self mainMenu]];
    [self.window makeKeyAndOrderFront:nil];
    [self.window makeFirstResponder:self.field];
    [NSApp activateIgnoringOtherApps:YES];
}

- (void)rename:(id)sender {
    self.window.title = self.field.stringValue;
}

- (void)clear:(id)sender {
    [self.paint clear];
}

- (void)info:(id)sender {
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"mac-paint";
    alert.informativeText = @"A programmatic AppKit app on EggOS.";
    [alert addButtonWithTitle:@"OK"];
    [alert addButtonWithTitle:@"Cancel"];
    [alert runModal];
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
}

@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSApplication *application = [NSApplication sharedApplication];
        [application setActivationPolicy:NSApplicationActivationPolicyRegular];
        AppDelegate *delegate = [[AppDelegate alloc] init];
        application.delegate = delegate;
        [application run];
    }
    return 0;
}
