using System;

class Program {
    static void Main() {
        string message = Greeter.greet("reader"); // DEFINITION
        Console.WriteLine(message.Length); // COMPLETION
        int broken = 1L; // DIAGNOSTIC
        Console.WriteLine(broken);
        Console.WriteLine("😀 é界");
    }
}
