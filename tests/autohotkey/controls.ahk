#Requires AutoHotkey v2.0
#NoTrayIcon
#SingleInstance Off

fruits := ComObject("Scripting.Dictionary")
fruits.Add("Apples", 3)
fruits.Add("Bananas", 5)
fruits.Add("Cherries", 7)
fruits.Add("Dates", 11)
fruits.Add("Elderberries", 13)

main := Gui("-DPIScale", "EggOS controls")
main.OnEvent("Close", (*) => ExitApp())

toolbar := main.AddCustom("ClassToolbarWindow32 x0 y0 w480 h30 0x184C")
labels := ["Step", "Reset", "Show tree"]
strings := Buffer(64, 0)
offset := 0
for label in labels
    offset += StrPut(label, strings.Ptr + offset, "UTF-16")
SendMessage(0x041E, 32, 0, toolbar)
first := SendMessage(0x044D, 0, strings.Ptr, toolbar)
buttons := Buffer(32 * labels.Length, 0)
for index, label in labels {
    at := (index - 1) * 32
    NumPut("Int", -2, "Int", 1000 + index, "UChar", 4, "UChar", 0x10, buttons, at)
    NumPut("Ptr", first + index - 1, buttons, at + 24)
}
SendMessage(0x0444, labels.Length, buttons.Ptr, toolbar)
SendMessage(0x0421, 0, 0, toolbar)

tabs := main.AddTab2("x8 y36 w464 h240", ["List", "Tree"])
tabs.UseTab(1)
fruitList := main.AddListView("x12 y64 w456 h208 -Multi", ["Fruit", "Count"])
for name in fruits.Keys()
    fruitList.Add(, name, fruits.Item(name))
fruitList.ModifyCol(1, 200)
fruitList.ModifyCol(2, "80 Integer")
fruitList.Modify(1, "Select Focus")
tabs.UseTab(2)
tree := main.AddTreeView("x12 y64 w456 h208")
fruit := tree.Add("Fruit")
for name in fruits.Keys()
    tree.Add(name, fruit)
vegetables := tree.Add("Vegetables")
tree.Add("Carrots", vegetables)
tree.Add("Leeks", vegetables)
tree.Modify(fruit, "Select")
tabs.UseTab()

progress := main.AddProgress("x8 y282 w424 h20 Range0-100", 40)
main.AddPicture("x440 y276", "egg.png")
status := main.AddStatusBar()
status.SetParts(240)
status.SetText("Ready", 1)
status.SetText(fruits.Count " items", 2)

fruitList.OnEvent("ItemSelect", ShowFruit)
tree.OnEvent("ItemSelect", ShowItem)
OnMessage(0x0111, ToolbarCommand)
main.Show("w480 h360")
fruitList.Focus()

ShowFruit(control, row, selected) {
    if selected
        status.SetText(control.GetText(row, 1) " " control.GetText(row, 2), 1)
}

ShowItem(control, item) {
    status.SetText(control.GetText(item), 1)
}

ToolbarCommand(wParam, lParam, message, window) {
    if lParam != toolbar.Hwnd
        return
    switch wParam & 0xFFFF {
        case 1001:
            progress.Value += 20
            status.SetText("Progress " progress.Value, 1)
        case 1002:
            progress.Value := 0
            status.SetText("Progress 0", 1)
        case 1003:
            tabs.Value := 2
            status.SetText("Tree", 1)
    }
}
