using System;
using System.Text;

class Program {
    static void Main() {
        StringBuilder text = new(); // LIBRARY_CLASS
        text.Append("😀 é界"); // LIBRARY_METHOD
        GeneratedWidget widget = new(); // GENERATED_CLASS
        Console.WriteLine(GeneratedWidget.Message()); // GENERATED_METHOD
        int broken = 1L; // DIAGNOSTIC
    }
}
