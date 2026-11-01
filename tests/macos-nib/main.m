#import <AppKit/AppKit.h>

@interface NibDelegate : NSObject <NSApplicationDelegate>
@property(strong) IBOutlet NSWindow *window;
@property(strong) IBOutlet NSTextField *label;
@property(strong) IBOutlet NSTextField *field;
@end

@implementation NibDelegate

- (void)awakeFromNib {
    NSLog(@"mac-nib: awake, window %@", self.window.title);
}

- (IBAction)greet:(id)sender {
    NSString *name = self.field.stringValue.length > 0 ? self.field.stringValue : @"nib";
    self.label.stringValue = [NSString stringWithFormat:@"Hello, %@", name];
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)application {
    return YES;
}

@end

int main(int argc, const char *argv[]) {
    return NSApplicationMain(argc, argv);
}
