using UnrealBuildTool;

public class QEMLevelSceneSimplifier : ModuleRules
{
    public QEMLevelSceneSimplifier(ReadOnlyTargetRules Target) : base(Target)
    {
        PCHUsage = ModuleRules.PCHUsageMode.UseExplicitOrSharedPCHs;

        PrivateDependencyModuleNames.AddRange(
            new string[]
            {
                "Core",
                "CoreUObject",
                "Engine",
                "InputCore",
                "Slate",
                "SlateCore",
                "EditorFramework",
                "UnrealEd",
                "AssetTools",
                "LevelEditor",
                "ToolMenus",
                "Projects",
                "MeshDescription",
                "StaticMeshDescription",
                "RenderCore",
                "MeshReductionInterface",
                "MeshUtilitiesCommon",
                "QEMSimplifierLevel"
            }
        );
    }
}
