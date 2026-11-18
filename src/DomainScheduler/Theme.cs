using System;
using System.Windows;
using System.Windows.Markup;

namespace DomainScheduler.App;

/// <summary>
/// The HUD control styles (1.14.0). Every style is its own dictionary, parsed on its own: a style that fails to load
/// leaves the others working (the control then keeps the Windows look) and writes one warning to the log.
/// </summary>
internal static class Theme
{
	private const string Head = "<ResourceDictionary xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\" xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\">\n";

	private const string Tail = "\n</ResourceDictionary>";

	// Buttons: square-ish, thin line; hover = a faint pink tint over the button's own background and a pink line.
	private const string ButtonXaml = """
  <Style TargetType="Button">
    <Setter Property="Foreground" Value="{DynamicResource Text}"/>
    <Setter Property="Background" Value="{DynamicResource Panel2}"/>
    <Setter Property="BorderBrush" Value="{DynamicResource Line}"/>
    <Setter Property="BorderThickness" Value="1"/>
    <Setter Property="Padding" Value="12,0"/>
    <Setter Property="FocusVisualStyle" Value="{x:Null}"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="Button">
          <Grid x:Name="root">
            <Border x:Name="b" Background="{TemplateBinding Background}" BorderBrush="{TemplateBinding BorderBrush}" BorderThickness="{TemplateBinding BorderThickness}" CornerRadius="3"/>
            <Border x:Name="h" Background="{DynamicResource Hover}" CornerRadius="3" Visibility="Hidden" IsHitTestVisible="False"/>
            <ContentPresenter Margin="{TemplateBinding Padding}" HorizontalAlignment="{TemplateBinding HorizontalContentAlignment}" VerticalAlignment="{TemplateBinding VerticalContentAlignment}" RecognizesAccessKey="False"/>
          </Grid>
          <ControlTemplate.Triggers>
            <Trigger Property="IsMouseOver" Value="True">
              <Setter TargetName="h" Property="Visibility" Value="Visible"/>
              <Setter TargetName="b" Property="BorderBrush" Value="{DynamicResource AccentLine}"/>
            </Trigger>
            <Trigger Property="IsKeyboardFocused" Value="True">
              <Setter TargetName="b" Property="BorderBrush" Value="{DynamicResource Accent}"/>
            </Trigger>
            <Trigger Property="IsPressed" Value="True">
              <Setter TargetName="root" Property="Opacity" Value="0.8"/>
            </Trigger>
            <Trigger Property="IsEnabled" Value="False">
              <Setter TargetName="root" Property="Opacity" Value="0.45"/>
            </Trigger>
          </ControlTemplate.Triggers>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
    <Setter Property="VerticalContentAlignment" Value="Center"/>
    <Setter Property="HorizontalContentAlignment" Value="Center"/>
  </Style>
""";

	private const string ComboXaml = """
  <Style TargetType="ComboBoxItem">
    <Setter Property="Foreground" Value="{DynamicResource Text}"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="ComboBoxItem">
          <Border x:Name="b" Background="{DynamicResource None}" Padding="9,6" CornerRadius="2">
            <ContentPresenter/>
          </Border>
          <ControlTemplate.Triggers>
            <Trigger Property="IsHighlighted" Value="True">
              <Setter TargetName="b" Property="Background" Value="{DynamicResource Sel}"/>
            </Trigger>
          </ControlTemplate.Triggers>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
  <Style TargetType="ComboBox">
    <Setter Property="Foreground" Value="{DynamicResource Strong}"/>
    <Setter Property="MaxDropDownHeight" Value="320"/>
    <Setter Property="FocusVisualStyle" Value="{x:Null}"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="ComboBox">
          <Grid>
            <ToggleButton Focusable="False" ClickMode="Press" IsChecked="{Binding IsDropDownOpen, Mode=TwoWay, RelativeSource={RelativeSource TemplatedParent}}">
              <ToggleButton.Template>
                <ControlTemplate TargetType="ToggleButton">
                  <Border x:Name="b" Background="{DynamicResource Panel2}" BorderBrush="{DynamicResource Line}" BorderThickness="1" CornerRadius="3">
                    <Path HorizontalAlignment="Right" VerticalAlignment="Center" Margin="0,0,10,0" Data="M0,0 L4,4 L8,0" Stroke="{DynamicResource Accent}" StrokeThickness="1.2"/>
                  </Border>
                  <ControlTemplate.Triggers>
                    <Trigger Property="IsMouseOver" Value="True">
                      <Setter TargetName="b" Property="BorderBrush" Value="{DynamicResource AccentLine}"/>
                      <Setter TargetName="b" Property="Background" Value="{DynamicResource Hover}"/>
                    </Trigger>
                    <Trigger Property="IsChecked" Value="True">
                      <Setter TargetName="b" Property="BorderBrush" Value="{DynamicResource Accent}"/>
                    </Trigger>
                  </ControlTemplate.Triggers>
                </ControlTemplate>
              </ToggleButton.Template>
            </ToggleButton>
            <ContentPresenter IsHitTestVisible="False" Margin="10,0,26,0" VerticalAlignment="Center" HorizontalAlignment="Left" Content="{TemplateBinding SelectionBoxItem}"/>
            <Popup x:Name="PART_Popup" Placement="Bottom" IsOpen="{TemplateBinding IsDropDownOpen}" AllowsTransparency="True" Focusable="False" PopupAnimation="None">
              <Border MinWidth="{TemplateBinding ActualWidth}" MaxHeight="{TemplateBinding MaxDropDownHeight}" Margin="0,3,0,0" Background="{DynamicResource Pop}" BorderBrush="{DynamicResource AccentLine}" BorderThickness="1" CornerRadius="3" Padding="3">
                <ScrollViewer VerticalScrollBarVisibility="Auto">
                  <ItemsPresenter/>
                </ScrollViewer>
              </Border>
            </Popup>
          </Grid>
          <ControlTemplate.Triggers>
            <Trigger Property="IsEnabled" Value="False">
              <Setter Property="Opacity" Value="0.45"/>
            </Trigger>
          </ControlTemplate.Triggers>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
""";

	private const string ScrollXaml = """
  <Style x:Key="BarThumb" TargetType="Thumb">
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="Thumb">
          <Border x:Name="t" CornerRadius="1" Background="{DynamicResource Line}" Margin="1,0"/>
          <ControlTemplate.Triggers>
            <Trigger Property="IsMouseOver" Value="True">
              <Setter TargetName="t" Property="Background" Value="{DynamicResource Dim}"/>
            </Trigger>
            <Trigger Property="IsDragging" Value="True">
              <Setter TargetName="t" Property="Background" Value="{DynamicResource Accent}"/>
            </Trigger>
          </ControlTemplate.Triggers>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
  <Style x:Key="BarPage" TargetType="RepeatButton">
    <Setter Property="Focusable" Value="False"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="RepeatButton">
          <Border Background="{DynamicResource None}"/>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
  <Style TargetType="ScrollBar">
    <Setter Property="Width" Value="7"/>
    <Setter Property="MinWidth" Value="7"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="ScrollBar">
          <Track x:Name="PART_Track" IsDirectionReversed="True">
            <Track.DecreaseRepeatButton>
              <RepeatButton Style="{StaticResource BarPage}" Command="ScrollBar.PageUpCommand"/>
            </Track.DecreaseRepeatButton>
            <Track.Thumb>
              <Thumb Style="{StaticResource BarThumb}"/>
            </Track.Thumb>
            <Track.IncreaseRepeatButton>
              <RepeatButton Style="{StaticResource BarPage}" Command="ScrollBar.PageDownCommand"/>
            </Track.IncreaseRepeatButton>
          </Track>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
    <Style.Triggers>
      <Trigger Property="Orientation" Value="Horizontal">
        <Setter Property="Width" Value="Auto"/>
        <Setter Property="MinWidth" Value="0"/>
        <Setter Property="Height" Value="7"/>
        <Setter Property="MinHeight" Value="7"/>
        <Setter Property="Template">
          <Setter.Value>
            <ControlTemplate TargetType="ScrollBar">
              <Track x:Name="PART_Track">
                <Track.DecreaseRepeatButton>
                  <RepeatButton Style="{StaticResource BarPage}" Command="ScrollBar.PageLeftCommand"/>
                </Track.DecreaseRepeatButton>
                <Track.Thumb>
                  <Thumb Style="{StaticResource BarThumb}"/>
                </Track.Thumb>
                <Track.IncreaseRepeatButton>
                  <RepeatButton Style="{StaticResource BarPage}" Command="ScrollBar.PageRightCommand"/>
                </Track.IncreaseRepeatButton>
              </Track>
            </ControlTemplate>
          </Setter.Value>
        </Setter>
      </Trigger>
    </Style.Triggers>
  </Style>
""";

	private const string TipXaml = """
  <Style TargetType="ToolTip">
    <Setter Property="Foreground" Value="{DynamicResource Text}"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="ToolTip">
          <Border Background="{DynamicResource Pop}" BorderBrush="{DynamicResource AccentLine}" BorderThickness="1" CornerRadius="3" Padding="9,6">
            <ContentPresenter/>
          </Border>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
""";

	private const string TextBoxXaml = """
  <Style TargetType="TextBox">
    <Setter Property="FocusVisualStyle" Value="{x:Null}"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="TextBox">
          <Border x:Name="b" Background="{TemplateBinding Background}" BorderBrush="{TemplateBinding BorderBrush}" BorderThickness="1" CornerRadius="3">
            <ScrollViewer x:Name="PART_ContentHost" Margin="{TemplateBinding Padding}" VerticalAlignment="{TemplateBinding VerticalContentAlignment}" Focusable="False" HorizontalScrollBarVisibility="Hidden" VerticalScrollBarVisibility="Hidden"/>
          </Border>
          <ControlTemplate.Triggers>
            <Trigger Property="IsMouseOver" Value="True">
              <Setter TargetName="b" Property="BorderBrush" Value="{DynamicResource AccentLine}"/>
            </Trigger>
            <Trigger Property="IsKeyboardFocused" Value="True">
              <Setter TargetName="b" Property="BorderBrush" Value="{DynamicResource Accent}"/>
            </Trigger>
            <Trigger Property="IsEnabled" Value="False">
              <Setter TargetName="b" Property="Opacity" Value="0.45"/>
            </Trigger>
          </ControlTemplate.Triggers>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
""";

	// A thin track, the part up to the value in pink, a square thumb. (The graphics card page draws its own
	// segmented slider; this one is for any other slider.)
	private const string SliderXaml = """
  <Style x:Key="SliderThumb" TargetType="Thumb">
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="Thumb">
          <Border x:Name="t" Width="12" Height="12" Background="{DynamicResource Bg}" BorderBrush="{DynamicResource Accent}" BorderThickness="1.5"/>
          <ControlTemplate.Triggers>
            <Trigger Property="IsMouseOver" Value="True">
              <Setter TargetName="t" Property="Background" Value="{DynamicResource Accent}"/>
            </Trigger>
            <Trigger Property="IsDragging" Value="True">
              <Setter TargetName="t" Property="Background" Value="{DynamicResource Accent}"/>
            </Trigger>
          </ControlTemplate.Triggers>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
  <Style x:Key="SliderFill" TargetType="RepeatButton">
    <Setter Property="Focusable" Value="False"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="RepeatButton">
          <Grid Background="{DynamicResource None}">
            <Border Height="2" VerticalAlignment="Center" Background="{DynamicResource Accent}"/>
          </Grid>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
  <Style x:Key="SliderRest" TargetType="RepeatButton">
    <Setter Property="Focusable" Value="False"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="RepeatButton">
          <Grid Background="{DynamicResource None}">
            <Border Height="2" VerticalAlignment="Center" Background="{DynamicResource Line}"/>
          </Grid>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
  <Style TargetType="Slider">
    <Setter Property="FocusVisualStyle" Value="{x:Null}"/>
    <Setter Property="Height" Value="22"/>
    <Setter Property="Template">
      <Setter.Value>
        <ControlTemplate TargetType="Slider">
          <Track x:Name="PART_Track">
            <Track.DecreaseRepeatButton>
              <RepeatButton Style="{StaticResource SliderFill}" Command="Slider.DecreaseLarge"/>
            </Track.DecreaseRepeatButton>
            <Track.Thumb>
              <Thumb Style="{StaticResource SliderThumb}"/>
            </Track.Thumb>
            <Track.IncreaseRepeatButton>
              <RepeatButton Style="{StaticResource SliderRest}" Command="Slider.IncreaseLarge"/>
            </Track.IncreaseRepeatButton>
          </Track>
          <ControlTemplate.Triggers>
            <Trigger Property="IsEnabled" Value="False">
              <Setter Property="Opacity" Value="0.45"/>
            </Trigger>
          </ControlTemplate.Triggers>
        </ControlTemplate>
      </Setter.Value>
    </Setter>
  </Style>
""";

	public static void Apply(Application app)
	{
		Ui.Register(app.Resources);
		foreach (var (name, xaml) in new (string, string)[]
		{
			("Button", ButtonXaml),
			("ComboBox", ComboXaml),
			("ScrollBar", ScrollXaml),
			("ToolTip", TipXaml),
			("TextBox", TextBoxXaml),
			("Slider", SliderXaml)
		})
		{
			try
			{
				ResourceDictionary item = (ResourceDictionary)XamlReader.Parse(Head + xaml + Tail);
				app.Resources.MergedDictionaries.Add(item);
			}
			catch (Exception ex)
			{
				AppLog.Add(L.F("Theme: {0}", name + ": " + ex.Message), AppLog.Sev.Warn);
			}
		}
	}
}
