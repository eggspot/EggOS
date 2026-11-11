#import <AppKit/AppKit.h>

static const NSRect CONTENT = {{0, 0}, {480, 360}};
static const NSRect GRID = {{16, 316}, {160, 28}};
static const NSRect SMALL = {{16, 284}, {160, 28}};
static const NSRect LARGE = {{16, 252}, {160, 28}};
static const NSRect BOLD = {{16, 212}, {96, 28}};
static const NSRect IMAGE = {{16, 100}, {96, 96}};
static const NSRect FIELD = {{16, 56}, {160, 28}};
static const NSRect SCROLL = {{192, 56}, {272, 288}};
static const NSRect COPY = {{16, 16}, {76, 28}};
static const NSRect PASTE = {{100, 16}, {76, 28}};
static const NSRect RESULT = {{192, 16}, {272, 28}};
static const NSInteger ROWS = 20;
static NSString *const CREDITS = @"Design by Claude Fable.\nEvery control here is drawn by the EggOS "
                                 @"toolkit, not by AppKit.\nNo third-party code, ever.\n\nThanks "
                                 @"to everyone who runs it.";

@interface ImageHost : NSImageView
@end

@implementation ImageHost

- (void)resetCursorRects {
    [self addCursorRect:self.bounds cursor:[NSCursor pointingHandCursor]];
}

@end

@interface Views : NSObject <NSApplicationDelegate, NSTableViewDataSource, NSTableViewDelegate> {
    NSWindow *_window;
    NSTableView *_table;
    NSTextField *_result;
    NSButton *_small;
    NSButton *_large;
}
@end

@implementation Views

- (NSString *)nameOfRow:(NSInteger)row {
    return [NSString stringWithFormat:@"Item %ld", (long)(row + 1)];
}

- (NSInteger)numberOfRowsInTableView:(NSTableView *)tableView {
    return ROWS;
}

- (id)tableView:(NSTableView *)tableView objectValueForTableColumn:(NSTableColumn *)column row:(NSInteger)row {
    if ([column.identifier isEqualToString:@"size"]) {
        return [NSString stringWithFormat:@"%ld KB", (long)(row + 1)];
    }
    return [self nameOfRow:row];
}

- (void)tableViewSelectionDidChange:(NSNotification *)notification {
    NSInteger row = _table.selectedRow;
    if (row < 0) {
        return;
    }
    _result.stringValue = [NSString stringWithFormat:@"Selected: %@", [self nameOfRow:row]];
}

- (void)gridToggled:(NSButton *)sender {
    NSTableViewGridLineStyle both =
        NSTableViewSolidVerticalGridLineMask | NSTableViewSolidHorizontalGridLineMask;
    _table.gridStyleMask = sender.state == NSControlStateValueOn ? both : NSTableViewGridNone;
}

- (void)sizeChosen:(NSButton *)sender {
    _small.state = sender == _small ? NSControlStateValueOn : NSControlStateValueOff;
    _large.state = sender == _large ? NSControlStateValueOn : NSControlStateValueOff;
}

- (void)boldToggled:(NSButton *)sender {
}

- (void)rowClicked:(NSTableView *)sender {
}

- (void)rowOpened:(NSTableView *)sender {
}

- (void)copySelected:(NSButton *)sender {
    NSInteger row = _table.selectedRow;
    if (row < 0) {
        return;
    }
    NSPasteboard *pasteboard = [NSPasteboard generalPasteboard];
    [pasteboard clearContents];
    [pasteboard setString:[self nameOfRow:row] forType:NSPasteboardTypeString];
}

- (void)pasteToLabel:(NSButton *)sender {
    NSString *text = [[NSPasteboard generalPasteboard] stringForType:NSPasteboardTypeString];
    _result.stringValue = text ? [@"Pasted: " stringByAppendingString:text] : @"Pasted:";
}

- (void)showAbout:(id)sender {
    NSMutableDictionary *options = [NSMutableDictionary dictionary];
    NSImage *icon = [NSImage imageNamed:@"egg"];
    if (icon) {
        options[@"ApplicationIcon"] = icon;
    }
    options[@"Credits"] = CREDITS;
    [NSApp orderFrontStandardAboutPanelWithOptions:options];
}

- (NSButton *)button:(NSString *)title frame:(NSRect)frame action:(SEL)action {
    NSButton *button = [NSButton buttonWithTitle:title target:self action:action];
    button.frame = frame;
    return button;
}

- (NSMenu *)mainMenu {
    NSMenu *bar = [[NSMenu alloc] initWithTitle:@""];
    NSMenuItem *appItem = [[NSMenuItem alloc] initWithTitle:@"" action:nil keyEquivalent:@""];
    NSMenu *appMenu = [[NSMenu alloc] initWithTitle:@"mac-views"];
    [appMenu addItemWithTitle:@"About mac-views" action:@selector(showAbout:) keyEquivalent:@""];
    [appMenu addItem:[NSMenuItem separatorItem]];
    [appMenu addItemWithTitle:@"Quit mac-views" action:@selector(terminate:) keyEquivalent:@"q"];
    appItem.submenu = appMenu;
    [bar addItem:appItem];
    NSMenuItem *editItem = [[NSMenuItem alloc] initWithTitle:@"" action:nil keyEquivalent:@""];
    NSMenu *edit = [[NSMenu alloc] initWithTitle:@"Edit"];
    [edit addItemWithTitle:@"Cut" action:@selector(cut:) keyEquivalent:@"x"];
    [edit addItemWithTitle:@"Copy" action:@selector(copy:) keyEquivalent:@"c"];
    [edit addItemWithTitle:@"Paste" action:@selector(paste:) keyEquivalent:@"v"];
    [edit addItem:[NSMenuItem separatorItem]];
    [edit addItemWithTitle:@"Select All" action:@selector(selectAll:) keyEquivalent:@"a"];
    editItem.submenu = edit;
    [bar addItem:editItem];
    return bar;
}

- (NSTableView *)table {
    NSTableView *table = [[NSTableView alloc] initWithFrame:NSMakeRect(0, 0, 0, 0)];
    NSTableColumn *name = [[NSTableColumn alloc] initWithIdentifier:@"name"];
    name.title = @"Name";
    name.width = 160;
    NSTableColumn *size = [[NSTableColumn alloc] initWithIdentifier:@"size"];
    size.title = @"Size";
    size.width = 100;
    [table addTableColumn:name];
    [table addTableColumn:size];
    table.usesAlternatingRowBackgroundColors = YES;
    table.dataSource = self;
    table.delegate = self;
    table.target = self;
    table.action = @selector(rowClicked:);
    table.doubleAction = @selector(rowOpened:);
    [table reloadData];
    return table;
}

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    NSApp.mainMenu = [self mainMenu];
    _window = [[NSWindow alloc] initWithContentRect:CONTENT
                                          styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                                    NSWindowStyleMaskMiniaturizable
                                            backing:NSBackingStoreBuffered
                                              defer:NO];
    _window.title = @"mac-views";
    NSView *content = _window.contentView;

    NSButton *grid = [NSButton checkboxWithTitle:@"Show grid" target:self action:@selector(gridToggled:)];
    grid.frame = GRID;
    [content addSubview:grid];

    _small = [NSButton radioButtonWithTitle:@"Small" target:self action:@selector(sizeChosen:)];
    _small.frame = SMALL;
    _small.state = NSControlStateValueOn;
    [content addSubview:_small];
    _large = [NSButton radioButtonWithTitle:@"Large" target:self action:@selector(sizeChosen:)];
    _large.frame = LARGE;
    [content addSubview:_large];

    NSButton *bold = [self button:@"Bold" frame:BOLD action:@selector(boldToggled:)];
    [bold setButtonType:NSButtonTypePushOnPushOff];
    [content addSubview:bold];

    ImageHost *image = [[ImageHost alloc] initWithFrame:IMAGE];
    image.image = [NSImage imageNamed:@"egg"];
    [content addSubview:image];

    NSTextField *field = [[NSTextField alloc] initWithFrame:FIELD];
    field.placeholderString = @"Type here";
    [content addSubview:field];

    NSScrollView *scroll = [[NSScrollView alloc] initWithFrame:SCROLL];
    scroll.borderType = NSBezelBorder;
    scroll.hasVerticalScroller = YES;
    _table = [self table];
    scroll.documentView = _table;
    [content addSubview:scroll];

    [content addSubview:[self button:@"Copy" frame:COPY action:@selector(copySelected:)]];
    [content addSubview:[self button:@"Paste" frame:PASTE action:@selector(pasteToLabel:)]];

    _result = [NSTextField labelWithString:@"Pasted:"];
    _result.frame = RESULT;
    [content addSubview:_result];

    [_window makeKeyAndOrderFront:nil];
    [_window makeFirstResponder:field];
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
}

@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSApplication *application = [NSApplication sharedApplication];
        Views *views = [[Views alloc] init];
        application.delegate = views;
        [application run];
    }
    return 0;
}
