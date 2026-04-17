using UnrealBuildTool;
using System.IO;

public class QEMSimplifierLevel : ModuleRules
{
    public QEMSimplifierLevel(ReadOnlyTargetRules Target) : base(Target)
    {
        Type = ModuleType.External;

        string ThirdPartyRoot = ModuleDirectory;
        string IncludeDir = Path.Combine(ThirdPartyRoot, "Include");
        string LibRoot = Path.Combine(ThirdPartyRoot, "Lib");
        string BinRoot = Path.Combine(ThirdPartyRoot, "Binary");

        PublicSystemIncludePaths.Add(IncludeDir);

        if (Target.Platform == UnrealTargetPlatform.Win64)
        {
            string LibFile = Path.Combine(LibRoot, "Win64", "qem_simplifier.dll.lib");
            string DllFile = Path.Combine(BinRoot, "Win64", "qem_simplifier.dll");
            string PluginBinDir = Path.Combine(PluginDirectory, "Binaries", "Win64");
            string PluginDllFile = Path.Combine(PluginBinDir, "qem_simplifier.dll");

            PublicAdditionalLibraries.Add(LibFile);
            PublicDelayLoadDLLs.Add("qem_simplifier.dll");
            PublicRuntimeLibraryPaths.Add(PluginBinDir);
            RuntimeDependencies.Add(PluginDllFile, DllFile);
        }
        else if (Target.Platform == UnrealTargetPlatform.Linux)
        {
            string StaticLib = Path.Combine(LibRoot, "Linux", "libqem_simplifier.a");
            string SharedLib = Path.Combine(LibRoot, "Linux", "libqem_simplifier.so");
            PublicAdditionalLibraries.Add(StaticLib);
            PublicAdditionalLibraries.Add(SharedLib);
            RuntimeDependencies.Add(SharedLib);
        }
        else if (Target.Platform == UnrealTargetPlatform.Mac)
        {
            string StaticLib = Path.Combine(LibRoot, "Mac", "libqem_simplifier.a");
            string Dylib = Path.Combine(BinRoot, "Mac", "libqem_simplifier.dylib");
            PublicAdditionalLibraries.Add(StaticLib);
            PublicAdditionalLibraries.Add(Dylib);
            RuntimeDependencies.Add(Dylib);
        }
    }
}
