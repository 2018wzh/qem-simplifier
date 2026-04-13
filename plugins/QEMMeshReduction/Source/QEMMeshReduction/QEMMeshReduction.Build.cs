using UnrealBuildTool;

public class QEMMeshReduction : ModuleRules
{
    public QEMMeshReduction(ReadOnlyTargetRules Target) : base(Target)
    {
        bWarningsAsErrors = false;
        PCHUsage = ModuleRules.PCHUsageMode.UseExplicitOrSharedPCHs;

        PrivateDependencyModuleNames.AddRange(
            new string[]
            {
                "Core",
                "CoreUObject",
                "Engine",
                "Projects",
                "RenderCore",
                "NaniteUtilities",
                "MeshReductionInterface",
                "MeshUtilitiesCommon",
                "MeshDescription",
                "StaticMeshDescription",
                "QEMSimplifier",
            }
        );

    }
}
