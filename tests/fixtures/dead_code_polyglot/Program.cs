using System;

public class Widget : IDisposable
{
    [HttpGet]
    public string Get()
    {
        return "";
    }

    public void Dispose()
    {
    }

    public override string ToString()
    {
        return "w";
    }

    public void CsUnusedHelper()
    {
    }
}
